//! The public, read-only side of the Registry: the realm list a player browses. Bounded queries, opaque keyset cursors, no totals, no secrets.

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::caps::Ruleset;
use crate::sign::RealmId;
use crate::{
    invalid, AccountProvisioning, ModuleEntry, Population, ProtoError, Rates, Result,
    REGISTRY_PROTOCOL_VERSION,
};

pub const DEFAULT_PAGE: u32 = 50;
pub const MAX_PAGE: u32 = 100;
pub const MAX_SEARCH_CHARS: usize = 64;
/// The longest description the list carries; the detail has the whole text.
pub const SUMMARY_DESCRIPTION_CHARS: usize = 160;
const MAX_CURSOR_BYTES: usize = 300;
const MAX_QUERY_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortKey {
    Name,
    Players,
    Cap,
    Created,
}

impl SortKey {
    pub fn as_str(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Players => "players",
            SortKey::Cap => "cap",
            SortKey::Created => "created",
        }
    }
    /// Names read A to Z, the rest biggest first.
    pub fn default_order(self) -> Order {
        match self {
            SortKey::Name => Order::Asc,
            _ => Order::Desc,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    Asc,
    Desc,
}

impl Order {
    pub fn as_str(self) -> &'static str {
        match self {
            Order::Asc => "asc",
            Order::Desc => "desc",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Online,
    Offline,
    All,
}

/// Where the previous page ended. Opaque to clients; it names the sort it belongs to, so it cannot be replayed against another ordering.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    pub s: SortKey,
    pub o: Order,
    /// The sort value of the last row: text for the name, a number for the others.
    pub k: CursorKey,
    pub id: RealmId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CursorKey {
    Num(i64),
    Text(String),
}

impl Cursor {
    pub fn encode(&self) -> String {
        B64.encode(serde_json::to_vec(self).expect("a cursor serialises"))
    }

    pub fn decode(text: &str) -> Result<Self> {
        if text.len() > MAX_CURSOR_BYTES {
            return invalid("the cursor is too long");
        }
        let bytes = B64
            .decode(text)
            .map_err(|_| ProtoError::Invalid("the cursor is not valid".into()))?;
        let c: Cursor = serde_json::from_slice(&bytes)
            .map_err(|_| ProtoError::Invalid("the cursor is not valid".into()))?;
        let fits = matches!((c.s, &c.k), (SortKey::Name, CursorKey::Text(t)) if t.chars().count() <= crate::MAX_DISPLAY_NAME_CHARS)
            || matches!(
                (c.s, &c.k),
                (
                    SortKey::Players | SortKey::Cap | SortKey::Created,
                    CursorKey::Num(_)
                )
            );
        if !fits {
            return invalid("the cursor does not fit its sort");
        }
        Ok(c)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListQuery {
    pub limit: u32,
    pub cursor: Option<Cursor>,
    pub q: Option<String>,
    pub ruleset: Option<Ruleset>,
    pub cap_min: Option<u32>,
    pub cap_max: Option<u32>,
    pub module: Option<String>,
    pub players_min: Option<u32>,
    pub language: Option<String>,
    pub region: Option<String>,
    pub status: Status,
    pub sort: SortKey,
    pub order: Order,
}

impl Default for ListQuery {
    fn default() -> Self {
        Self {
            limit: DEFAULT_PAGE,
            cursor: None,
            q: None,
            ruleset: None,
            cap_min: None,
            cap_max: None,
            module: None,
            players_min: None,
            language: None,
            region: None,
            status: Status::Online,
            sort: SortKey::Players,
            order: Order::Desc,
        }
    }
}

fn percent_decode(s: &str) -> Result<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = s
                    .get(i + 1..i + 3)
                    .ok_or_else(|| ProtoError::Invalid("a percent escape is cut short".into()))?;
                out.push(
                    u8::from_str_radix(hex, 16)
                        .map_err(|_| ProtoError::Invalid("a percent escape is not hex".into()))?,
                );
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| ProtoError::Invalid("the query is not UTF-8".into()))
}

fn number(name: &str, v: &str, max: u32) -> Result<u32> {
    match v.parse::<u32>() {
        Ok(n) if n <= max => Ok(n),
        _ => invalid(format!("{name} is not a number from 0 to {max}")),
    }
}

impl ListQuery {
    /// A strict parse: an unknown parameter, a repeated one or an out-of-range value is an error, never silently ignored.
    pub fn parse(raw: Option<&str>) -> Result<Self> {
        let raw = raw.unwrap_or("");
        if raw.len() > MAX_QUERY_BYTES {
            return invalid("the query is too long");
        }
        let mut q = ListQuery::default();
        let mut seen = std::collections::BTreeSet::new();
        let mut order_given = false;
        for pair in raw.split('&').filter(|p| !p.is_empty()) {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            let (name, value) = (percent_decode(name)?, percent_decode(value)?);
            if !seen.insert(name.clone()) {
                return invalid(format!("{name} is given twice"));
            }
            if value.chars().any(char::is_control) {
                return invalid("a parameter has control characters");
            }
            match name.as_str() {
                "limit" => {
                    q.limit = number("limit", &value, MAX_PAGE).and_then(|n| {
                        if n == 0 {
                            invalid("limit is 0")
                        } else {
                            Ok(n)
                        }
                    })?
                }
                "cursor" => q.cursor = Some(Cursor::decode(&value)?),
                "q" => {
                    if value.chars().count() > MAX_SEARCH_CHARS {
                        return invalid("the search text is too long");
                    }
                    q.q = (!value.trim().is_empty()).then(|| value.trim().to_string());
                }
                "ruleset" => {
                    q.ruleset = Some(match value.as_str() {
                        "coa" => Ruleset::Coa,
                        "wildcard" => Ruleset::Wildcard,
                        _ => return invalid("ruleset is coa or wildcard"),
                    })
                }
                "cap_min" => q.cap_min = Some(number("cap_min", &value, 255)?),
                "cap_max" => q.cap_max = Some(number("cap_max", &value, 255)?),
                "module" => {
                    crate::validate_module_id(&value)?;
                    q.module = Some(value);
                }
                "players_min" => {
                    q.players_min = Some(number("players_min", &value, crate::MAX_PLAYER_NUMBER)?)
                }
                "language" => {
                    crate::validate_language(&value)?;
                    q.language = Some(value);
                }
                "region" => {
                    crate::validate_region(&value)?;
                    q.region = Some(value);
                }
                "status" => {
                    q.status = match value.as_str() {
                        "online" => Status::Online,
                        "offline" => Status::Offline,
                        "all" => Status::All,
                        _ => return invalid("status is online, offline or all"),
                    }
                }
                "sort" => {
                    q.sort = match value.as_str() {
                        "name" => SortKey::Name,
                        "players" => SortKey::Players,
                        "cap" => SortKey::Cap,
                        "created" => SortKey::Created,
                        _ => return invalid("sort is name, players, cap or created"),
                    }
                }
                "order" => {
                    order_given = true;
                    q.order = match value.as_str() {
                        "asc" => Order::Asc,
                        "desc" => Order::Desc,
                        _ => return invalid("order is asc or desc"),
                    }
                }
                other => return invalid(format!("unknown parameter {other}")),
            }
        }
        if !order_given {
            q.order = q.sort.default_order();
        }
        if let (Some(lo), Some(hi)) = (q.cap_min, q.cap_max) {
            if lo > hi {
                return invalid("cap_min is above cap_max");
            }
        }
        if let Some(c) = &q.cursor {
            if c.s != q.sort || c.o != q.order {
                return invalid("the cursor belongs to another sort");
            }
        }
        Ok(q)
    }

    /// A deterministic text of the whole query: the cache key and the base of a client's next request.
    pub fn canonical(&self) -> String {
        let mut parts = vec![
            format!("limit={}", self.limit),
            format!("sort={}", self.sort.as_str()),
            format!("order={}", self.order.as_str()),
        ];
        let status = match self.status {
            Status::Online => "online",
            Status::Offline => "offline",
            Status::All => "all",
        };
        parts.push(format!("status={status}"));
        let enc = |s: &str| {
            s.bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.') {
                        (b as char).to_string()
                    } else {
                        format!("%{b:02X}")
                    }
                })
                .collect::<String>()
        };
        if let Some(v) = &self.q {
            parts.push(format!("q={}", enc(v)));
        }
        if let Some(v) = self.ruleset {
            parts.push(format!("ruleset={}", v.as_str()));
        }
        if let Some(v) = self.cap_min {
            parts.push(format!("cap_min={v}"));
        }
        if let Some(v) = self.cap_max {
            parts.push(format!("cap_max={v}"));
        }
        if let Some(v) = &self.module {
            parts.push(format!("module={}", enc(v)));
        }
        if let Some(v) = self.players_min {
            parts.push(format!("players_min={v}"));
        }
        if let Some(v) = &self.language {
            parts.push(format!("language={}", enc(v)));
        }
        if let Some(v) = &self.region {
            parts.push(format!("region={}", enc(v)));
        }
        if let Some(c) = &self.cursor {
            parts.push(format!("cursor={}", c.encode()));
        }
        parts.join("&")
    }
}

/// One row of the public list: what the table shows, without the capabilities (the detail has them).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RealmSummary {
    pub realm_id: RealmId,
    pub display_name: String,
    /// At most [`SUMMARY_DESCRIPTION_CHARS`] characters.
    pub description: String,
    pub language: String,
    pub region: Option<String>,
    pub ruleset: Ruleset,
    pub level_cap: Option<u32>,
    pub rates: Rates,
    pub modules: Vec<ModuleEntry>,
    pub population: Population,
    pub account_provisioning: AccountProvisioning,
    pub manager_version: String,
    pub capabilities_hash: String,
    pub listing_hash: String,
    pub metadata_revision: u64,
    pub online: bool,
    pub created_at: i64,
    pub last_seen_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RealmPage {
    pub protocol_version: u32,
    pub realms: Vec<RealmSummary>,
    /// Present when there may be more; pass it back as `cursor` with the same query.
    pub next_cursor: Option<String>,
}

impl RealmPage {
    pub fn new(realms: Vec<RealmSummary>, next_cursor: Option<String>) -> Self {
        Self {
            protocol_version: REGISTRY_PROTOCOL_VERSION,
            realms,
            next_cursor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_is_parsed_strictly() {
        let q = ListQuery::parse(Some(
            "q=Descen%20sion&ruleset=coa&cap_min=60&module=playerbots&sort=name&limit=25",
        ))
        .unwrap();
        assert_eq!(
            (
                q.q.as_deref(),
                q.ruleset,
                q.cap_min,
                q.module.as_deref(),
                q.sort,
                q.order,
                q.limit
            ),
            (
                Some("Descen sion"),
                Some(Ruleset::Coa),
                Some(60),
                Some("playerbots"),
                SortKey::Name,
                Order::Asc,
                25
            )
        );
        let d = ListQuery::parse(None).unwrap();
        assert_eq!(
            (d.sort, d.order, d.status, d.limit),
            (SortKey::Players, Order::Desc, Status::Online, 50),
            "by default: online realms, most players first"
        );
        for bad in [
            "limit=0",
            "limit=101",
            "limit=x",
            "sort=ping",
            "order=up",
            "ruleset=normal",
            "status=banned",
            "unknown=1",
            "q=a&q=b",
            "cap_min=300",
            "cap_min=70&cap_max=60",
            "module=<b>",
            "language=<x>",
            "cursor=%%%",
            "q=%zz",
            "q=bad%00",
            &format!("q={}", "x".repeat(65)),
        ] {
            assert!(ListQuery::parse(Some(bad)).is_err(), "{bad}");
        }
        assert!(
            ListQuery::parse(Some(&"a=b&".repeat(400))).is_err(),
            "too long"
        );
    }

    #[test]
    fn a_cursor_belongs_to_one_sort_and_is_bounded() {
        let c = Cursor {
            s: SortKey::Players,
            o: Order::Desc,
            k: CursorKey::Num(12),
            id: RealmId::new(),
        };
        let text = c.encode();
        assert_eq!(Cursor::decode(&text).unwrap(), c);
        assert!(ListQuery::parse(Some(&format!("sort=players&cursor={text}"))).is_ok());
        assert!(
            ListQuery::parse(Some(&format!("sort=name&cursor={text}"))).is_err(),
            "another sort"
        );
        assert!(
            ListQuery::parse(Some(&format!("sort=players&order=asc&cursor={text}"))).is_err(),
            "another order"
        );
        assert!(Cursor::decode(&"A".repeat(400)).is_err());
        assert!(Cursor::decode("not base64 !").is_err());
        let wrong = Cursor {
            s: SortKey::Name,
            o: Order::Asc,
            k: CursorKey::Num(1),
            id: RealmId::new(),
        };
        assert!(
            Cursor::decode(&wrong.encode()).is_err(),
            "a number is not a name"
        );
    }

    #[test]
    fn the_canonical_form_is_one_per_query() {
        let a = ListQuery::parse(Some("sort=name&q=a%20b&ruleset=coa")).unwrap();
        let b = ListQuery::parse(Some("ruleset=coa&q=a+b&sort=name&order=asc&limit=50")).unwrap();
        assert_eq!(a.canonical(), b.canonical());
        assert_eq!(
            ListQuery::parse(Some(&a.canonical())).unwrap(),
            a,
            "and it parses back to the same query"
        );
    }
}
