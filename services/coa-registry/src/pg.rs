//! PostgreSQL storage and forward-only migrations.

use std::time::Duration;

use coa_registry_proto::caps::{AdvertisedCapabilities, Ruleset};
use coa_registry_proto::{AccountProvisioning, CursorKey, HeartbeatRequest, ListQuery, Listing, ModuleEntry, Order, Population, Rates, RealmId, RealmSummary, RegisterRequest, SortKey, Status};
use deadpool_postgres::{Manager, ManagerConfig, Object, Pool, RecyclingMethod, Runtime};
use tokio_postgres::{NoTls, Row};

use crate::store::*;

const MIGRATIONS: &[(i32, &str, &str)] = &[
    (1, "realms", include_str!("../migrations/0001_realms.sql")),
    (2, "protocol_v2", include_str!("../migrations/0002_protocol_v2.sql")),
    (3, "browse_indexes", include_str!("../migrations/0003_browse_indexes.sql")),
    (4, "created_whole_seconds", include_str!("../migrations/0004_created_whole_seconds.sql")),
];

pub struct PgConfig {
    pub host: String,
    pub port: u16,
    pub dbname: String,
    pub user: String,
    pub password: String,
    pub pool_size: usize,
    pub statement_timeout_ms: u64,
}

pub struct PgStore {
    pool: Pool,
}

fn backend(e: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(e.to_string())
}

impl PgStore {
    pub async fn connect(cfg: &PgConfig) -> Result<Self, String> {
        let mut pg = tokio_postgres::Config::new();
        pg.host(&cfg.host).port(cfg.port).dbname(&cfg.dbname).user(&cfg.user).password(&cfg.password);
        pg.connect_timeout(Duration::from_secs(5));
        pg.application_name("coa-registry");
        pg.options(&format!("-c statement_timeout={} -c idle_in_transaction_session_timeout=10000 -c lock_timeout=3000", cfg.statement_timeout_ms));
        let manager = Manager::from_config(pg, NoTls, ManagerConfig { recycling_method: RecyclingMethod::Fast });
        let pool = Pool::builder(manager)
            .max_size(cfg.pool_size)
            .wait_timeout(Some(Duration::from_secs(5)))
            .create_timeout(Some(Duration::from_secs(5)))
            .runtime(Runtime::Tokio1)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { pool })
    }

    pub(crate) async fn conn(&self) -> StoreResult<Object> {
        self.pool.get().await.map_err(backend)
    }

    /// Apply every migration the database has not seen. A database that is ahead of this binary is refused: migrations only go forward.
    pub async fn migrate(&self) -> Result<(), String> {
        let mut c = self.pool.get().await.map_err(|e| e.to_string())?;
        c.batch_execute("SELECT pg_advisory_lock(7300100)").await.map_err(|e| e.to_string())?;
        let result = Self::migrate_locked(&mut c).await;
        let _ = c.batch_execute("SELECT pg_advisory_unlock(7300100)").await;
        result
    }

    async fn migrate_locked(c: &mut Object) -> Result<(), String> {
        c.batch_execute("CREATE TABLE IF NOT EXISTS schema_migrations (version integer PRIMARY KEY, name text NOT NULL, applied_at timestamptz NOT NULL DEFAULT now())").await.map_err(|e| e.to_string())?;
        let applied: Vec<i32> = c.query("SELECT version FROM schema_migrations ORDER BY version", &[]).await.map_err(|e| e.to_string())?.iter().map(|r| r.get(0)).collect();
        let newest = MIGRATIONS.last().map(|m| m.0).unwrap_or(0);
        if let Some(ahead) = applied.iter().find(|v| **v > newest) {
            return Err(format!("the database is at migration {ahead}, this binary knows up to {newest}: refusing to start"));
        }
        for (version, name, sql) in MIGRATIONS {
            if applied.contains(version) {
                continue;
            }
            let tx = c.transaction().await.map_err(|e| e.to_string())?;
            tx.batch_execute(sql).await.map_err(|e| format!("migration {version} {name}: {}", e.as_db_error().map(|d| d.message().to_string()).unwrap_or_else(|| e.to_string())))?;
            tx.execute("INSERT INTO schema_migrations(version, name) VALUES ($1, $2)", &[version, name]).await.map_err(|e| e.to_string())?;
            tx.commit().await.map_err(|e| e.to_string())?;
            tracing::info!(version, name, "migration applied");
        }
        Ok(())
    }
}

pub(crate) const COLUMNS: &str = "realm_id::text, public_key, EXTRACT(EPOCH FROM created_at)::bigint, EXTRACT(EPOCH FROM updated_at)::bigint, EXTRACT(EPOCH FROM last_seen_at)::bigint, published, display_name, description, language, region, rates, modules, account_automatic, account_existing_only, manager_version, listing_hash, ruleset, level_cap, capabilities, capabilities_hash, players, bots, capacity, metadata_revision, last_request_ts, advert_version";

pub(crate) fn to_row(r: &Row) -> StoreResult<RealmRow> {
    let id: String = r.get(0);
    let key: Vec<u8> = r.get(1);
    let rates: serde_json::Value = r.get(10);
    let modules: serde_json::Value = r.get(11);
    let ruleset: String = r.get(16);
    let level_cap: Option<i32> = r.get(17);
    let caps: serde_json::Value = r.get(18);
    let players: Option<i32> = r.get(20);
    let bots: i32 = r.get(21);
    let capacity: Option<i32> = r.get(22);
    let revision: i64 = r.get(23);
    let version: i16 = r.get(25);
    let listing = Listing {
        display_name: r.get(6),
        description: r.get(7),
        language: r.get(8),
        region: r.get(9),
        rates: serde_json::from_value::<Rates>(rates).unwrap_or_default(),
        modules: serde_json::from_value::<Vec<ModuleEntry>>(modules).unwrap_or_default(),
        account_provisioning: AccountProvisioning { automatic: r.get(12), existing_only: r.get(13) },
        manager_version: r.get(14),
    };
    let stored_hash: Option<String> = r.get(15);
    Ok(RealmRow {
        realm_id: RealmId::parse(&id).map_err(backend)?,
        public_key: key.try_into().map_err(|_| StoreError::Backend("a stored key is not 32 bytes".into()))?,
        created_at: r.get(2),
        updated_at: r.get(3),
        last_seen_at: r.get(4),
        published: r.get(5),
        listing_hash: stored_hash.unwrap_or_else(|| listing.hash()),
        listing,
        ruleset: match ruleset.as_str() {
            "coa" => Ruleset::Coa,
            "wildcard" => Ruleset::Wildcard,
            other => return Err(StoreError::Backend(format!("a stored ruleset is unknown: {other}"))),
        },
        level_cap: level_cap.map(|n| n as u32),
        capabilities: serde_json::from_value::<AdvertisedCapabilities>(caps).map_err(backend)?,
        capabilities_hash: r.get(19),
        population: Population { players: players.unwrap_or(0) as u32, bots: bots as u32, capacity: capacity.map(|n| n as u32) },
        metadata_revision: revision as u64,
        last_request_ts: r.get(24),
        advert_version: version as u8,
    })
}

fn caps_json(c: &AdvertisedCapabilities) -> StoreResult<serde_json::Value> {
    serde_json::to_value(c).map_err(backend)
}

struct ListingColumns {
    rates: serde_json::Value,
    modules: serde_json::Value,
}

fn listing_columns(l: &Listing) -> StoreResult<ListingColumns> {
    Ok(ListingColumns { rates: serde_json::to_value(&l.rates).map_err(backend)?, modules: serde_json::to_value(&l.modules).map_err(backend)? })
}

/// The sort expressions; each has a matching partial index (migration 3), so a page is an index range scan, never a sort of the table.
fn sort_expr(sort: SortKey) -> &'static str {
    match sort {
        SortKey::Name => "lower(display_name)",
        SortKey::Players => "COALESCE(players, 0)::bigint",
        SortKey::Cap => "COALESCE(level_cap, 0)::bigint",
        SortKey::Created => "created_at",
    }
}

type Param = Box<dyn tokio_postgres::types::ToSql + Sync + Send>;

fn summary_from(r: &Row, now: i64, ttl: i64) -> StoreResult<RealmSummary> {
    let id: String = r.get(0);
    let ruleset: String = r.get(10);
    let level_cap: Option<i32> = r.get(11);
    let players: Option<i32> = r.get(12);
    let bots: i32 = r.get(13);
    let capacity: Option<i32> = r.get(14);
    let revision: i64 = r.get(15);
    let last_seen: i64 = r.get(18);
    Ok(RealmSummary {
        realm_id: RealmId::parse(&id).map_err(backend)?,
        display_name: r.get(1),
        description: r.get(2),
        language: r.get(3),
        region: r.get(4),
        ruleset: match ruleset.as_str() {
            "coa" => Ruleset::Coa,
            "wildcard" => Ruleset::Wildcard,
            other => return Err(StoreError::Backend(format!("a stored ruleset is unknown: {other}"))),
        },
        level_cap: level_cap.map(|n| n as u32),
        rates: serde_json::from_value::<Rates>(r.get(5)).unwrap_or_default(),
        modules: serde_json::from_value::<Vec<ModuleEntry>>(r.get(6)).unwrap_or_default(),
        population: Population { players: players.unwrap_or(0) as u32, bots: bots as u32, capacity: capacity.map(|n| n as u32) },
        account_provisioning: AccountProvisioning { automatic: r.get(7), existing_only: r.get(8) },
        manager_version: r.get(9),
        capabilities_hash: r.get(16),
        listing_hash: r.get(17),
        metadata_revision: revision as u64,
        online: now - last_seen <= ttl,
        created_at: r.get(19),
        last_seen_at: last_seen,
    })
}

impl PgStore {
    async fn list_rows(&self, q: &ListQuery, now: i64, ttl: i64) -> StoreResult<Vec<RealmSummary>> {
        let mut sql = String::from("SELECT realm_id::text, display_name, left(description, 160), language, region, rates, modules, account_automatic, account_existing_only, manager_version, ruleset, level_cap, players, bots, capacity, metadata_revision, capabilities_hash, COALESCE(listing_hash, ''), EXTRACT(EPOCH FROM last_seen_at)::bigint, EXTRACT(EPOCH FROM created_at)::bigint FROM realms WHERE published AND advert_version = 2");
        let mut params: Vec<Param> = Vec::new();
        let mut push = |p: Param, params: &mut Vec<Param>| -> usize {
            params.push(p);
            params.len()
        };
        if let Some(t) = &q.q {
            let n = push(Box::new(t.clone()), &mut params);
            sql += &format!(" AND strpos(lower(display_name), lower(${n})) > 0");
        }
        if let Some(r) = q.ruleset {
            let n = push(Box::new(r.as_str().to_string()), &mut params);
            sql += &format!(" AND ruleset = ${n}");
        }
        if let Some(c) = q.cap_min {
            let n = push(Box::new(c as i32), &mut params);
            sql += &format!(" AND COALESCE(level_cap, 0) >= ${n}");
        }
        if let Some(c) = q.cap_max {
            let n = push(Box::new(c as i32), &mut params);
            sql += &format!(" AND COALESCE(level_cap, 0) <= ${n}");
        }
        if let Some(m) = &q.module {
            let n = push(Box::new(serde_json::json!([{ "id": m, "enabled": true }])), &mut params);
            sql += &format!(" AND modules @> ${n}");
        }
        if let Some(p) = q.players_min {
            let n = push(Box::new(p as i32), &mut params);
            sql += &format!(" AND COALESCE(players, 0) >= ${n}");
        }
        if let Some(l) = &q.language {
            let n = push(Box::new(l.clone()), &mut params);
            sql += &format!(" AND language = ${n}");
        }
        if let Some(r) = &q.region {
            let n = push(Box::new(r.clone()), &mut params);
            sql += &format!(" AND region = ${n}");
        }
        match q.status {
            Status::Online => {
                let n = push(Box::new(now - ttl), &mut params);
                sql += &format!(" AND last_seen_at > to_timestamp(${n}::bigint)");
            }
            Status::Offline => {
                let n = push(Box::new(now - ttl), &mut params);
                sql += &format!(" AND last_seen_at <= to_timestamp(${n}::bigint)");
            }
            Status::All => {}
        }
        let expr = sort_expr(q.sort);
        let (cmp, dir) = if q.order == Order::Desc { ("<", "DESC") } else { (">", "ASC") };
        if let Some(c) = &q.cursor {
            let k = match &c.k {
                CursorKey::Num(n) => push(Box::new(*n), &mut params),
                CursorKey::Text(t) => push(Box::new(t.clone()), &mut params),
            };
            let id = push(Box::new(c.id.to_string()), &mut params);
            let value = if q.sort == SortKey::Created { format!("to_timestamp(${k}::bigint)") } else { format!("${k}") };
            sql += &format!(" AND ({expr}, realm_id) {cmp} ({value}, ${id}::text::uuid)");
        }
        let limit = push(Box::new(i64::from(q.limit) + 1), &mut params);
        sql += &format!(" ORDER BY {expr} {dir}, realm_id {dir} LIMIT ${limit}");
        let refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = params.iter().map(|p| &**p as &(dyn tokio_postgres::types::ToSql + Sync)).collect();
        let c = self.conn().await?;
        let rows = c.query(&sql, &refs).await.map_err(backend)?;
        rows.iter().map(|r| summary_from(r, now, ttl)).collect()
    }
}

impl Store for PgStore {
    async fn ping(&self) -> bool {
        match self.pool.get().await {
            Ok(c) => c.query_one("SELECT 1", &[]).await.is_ok(),
            Err(_) => false,
        }
    }

    async fn key_of(&self, id: &RealmId) -> StoreResult<Option<[u8; 32]>> {
        let c = self.conn().await?;
        let row = c.query_opt("SELECT public_key FROM realms WHERE realm_id = $1::text::uuid", &[&id.to_string()]).await.map_err(backend)?;
        row.map(|r| r.get::<_, Vec<u8>>(0).try_into().map_err(|_| StoreError::Backend("a stored key is not 32 bytes".into()))).transpose()
    }

    async fn get(&self, id: &RealmId) -> StoreResult<Option<RealmRow>> {
        let c = self.conn().await?;
        let row = c.query_opt(&format!("SELECT {COLUMNS} FROM realms WHERE realm_id = $1::text::uuid"), &[&id.to_string()]).await.map_err(backend)?;
        row.as_ref().map(to_row).transpose()
    }

    async fn register(&self, req: &RegisterRequest, key: [u8; 32], now: i64, ts: i64) -> StoreResult<(RealmRow, bool)> {
        let mut c = self.conn().await?;
        let id = req.realm_id.to_string();
        for _ in 0..3 {
            let tx = c.transaction().await.map_err(backend)?;
            let existing = tx.query_opt(&format!("SELECT {COLUMNS} FROM realms WHERE realm_id = $1::text::uuid FOR UPDATE"), &[&id]).await.map_err(backend)?;
            let existing = existing.as_ref().map(to_row).transpose()?;
            let (row, created) = plan_register(existing.as_ref(), req, key, now, ts)?;
            let caps = caps_json(&row.capabilities)?;
            let cols = listing_columns(&row.listing)?;
            let ruleset = row.ruleset.as_str();
            let (players, bots, capacity) = (row.population.players as i32, row.population.bots as i32, row.population.capacity.map(|n| n as i32));
            let level_cap = row.level_cap.map(|n| n as i32);
            let l = &row.listing;
            if created {
                let n = tx
                    .execute(
                        "INSERT INTO realms (realm_id, public_key, created_at, updated_at, last_seen_at, published, display_name, description, language, region, rates, modules, account_automatic, account_existing_only, manager_version, listing_hash, ruleset, level_cap, capabilities, capabilities_hash, players, bots, capacity, metadata_revision, last_request_ts, advert_version) \
                         VALUES ($1::text::uuid, $2, to_timestamp($3::bigint), to_timestamp($3::bigint), to_timestamp($3::bigint), true, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, 1, $21, 2) ON CONFLICT (realm_id) DO NOTHING",
                        &[&id, &row.public_key.as_slice(), &now, &l.display_name, &l.description, &l.language, &l.region, &cols.rates, &cols.modules, &l.account_provisioning.automatic, &l.account_provisioning.existing_only, &l.manager_version, &row.listing_hash, &ruleset, &level_cap, &caps, &row.capabilities_hash, &players, &bots, &capacity, &ts],
                    )
                    .await
                    .map_err(backend)?;
                if n == 0 {
                    tx.rollback().await.map_err(backend)?;
                    continue;
                }
            } else {
                tx.execute(
                    "UPDATE realms SET updated_at = to_timestamp($2::bigint), last_seen_at = to_timestamp($3::bigint), published = true, display_name = $4, description = $5, language = $6, region = $7, rates = $8, modules = $9, account_automatic = $10, account_existing_only = $11, manager_version = $12, listing_hash = $13, ruleset = $14, level_cap = $15, capabilities = $16, capabilities_hash = $17, players = $18, bots = $19, capacity = $20, metadata_revision = $21, last_request_ts = $22, advert_version = 2 WHERE realm_id = $1::text::uuid",
                    &[&id, &row.updated_at, &now, &l.display_name, &l.description, &l.language, &l.region, &cols.rates, &cols.modules, &l.account_provisioning.automatic, &l.account_provisioning.existing_only, &l.manager_version, &row.listing_hash, &ruleset, &level_cap, &caps, &row.capabilities_hash, &players, &bots, &capacity, &(row.metadata_revision as i64), &ts],
                )
                .await
                .map_err(backend)?;
            }
            tx.commit().await.map_err(backend)?;
            return Ok((row, created));
        }
        Err(StoreError::Backend("the registration kept racing another one".into()))
    }

    async fn heartbeat(&self, id: &RealmId, hb: &HeartbeatRequest, now: i64, ts: i64) -> StoreResult<HeartbeatOutcome> {
        let mut c = self.conn().await?;
        let text = id.to_string();
        let tx = c.transaction().await.map_err(backend)?;
        let r = tx
            .query_opt("SELECT published, last_request_ts, capabilities_hash, listing_hash, metadata_revision, advert_version, display_name, description, language, region, rates, modules, account_automatic, account_existing_only, manager_version FROM realms WHERE realm_id = $1::text::uuid FOR UPDATE", &[&text])
            .await
            .map_err(backend)?
            .ok_or(StoreError::Unknown)?;
        let revision: i64 = r.get(4);
        let version: i16 = r.get(5);
        let stored_listing_hash: Option<String> = r.get(3);
        let listing_hash = match stored_listing_hash {
            Some(h) => h,
            None => {
                let l = Listing {
                    display_name: r.get(6),
                    description: r.get(7),
                    language: r.get(8),
                    region: r.get(9),
                    rates: serde_json::from_value::<Rates>(r.get(10)).unwrap_or_default(),
                    modules: serde_json::from_value::<Vec<ModuleEntry>>(r.get(11)).unwrap_or_default(),
                    account_provisioning: AccountProvisioning { automatic: r.get(12), existing_only: r.get(13) },
                    manager_version: r.get(14),
                };
                l.hash()
            }
        };
        let view = HeartbeatView { published: r.get(0), last_request_ts: r.get(1), capabilities_hash: r.get(2), listing_hash, metadata_revision: revision as u64, advert_version: version as u8 };
        let plan = plan_heartbeat(&view, hb, ts)?;
        let (players, bots, capacity) = (hb.population.players as i32, hb.population.bots as i32, hb.population.capacity.map(|n| n as i32));
        tx.execute(
            "UPDATE realms SET last_seen_at = to_timestamp($2::bigint), last_request_ts = $3, players = $4, bots = $5, capacity = $6, listing_hash = $7, capabilities_hash = $8, metadata_revision = $9, updated_at = CASE WHEN $10 THEN to_timestamp($2::bigint) ELSE updated_at END WHERE realm_id = $1::text::uuid",
            &[&text, &now, &ts, &players, &bots, &capacity, &plan.listing_hash, &plan.capabilities_hash, &(plan.metadata_revision as i64), &plan.metadata_changed],
        )
        .await
        .map_err(backend)?;
        if let Some(l) = &plan.listing {
            let cols = listing_columns(l)?;
            tx.execute(
                "UPDATE realms SET display_name = $2, description = $3, language = $4, region = $5, rates = $6, modules = $7, account_automatic = $8, account_existing_only = $9, manager_version = $10, advert_version = 2 WHERE realm_id = $1::text::uuid",
                &[&text, &l.display_name, &l.description, &l.language, &l.region, &cols.rates, &cols.modules, &l.account_provisioning.automatic, &l.account_provisioning.existing_only, &l.manager_version],
            )
            .await
            .map_err(backend)?;
        }
        if let Some(caps) = &plan.capabilities {
            let json = caps_json(caps)?;
            let level_cap = caps.level_cap().map(|n| n as i32);
            tx.execute("UPDATE realms SET capabilities = $2, ruleset = $3, level_cap = $4 WHERE realm_id = $1::text::uuid", &[&text, &json, &caps.content.ruleset.as_str(), &level_cap]).await.map_err(backend)?;
        }
        tx.commit().await.map_err(backend)?;
        Ok(HeartbeatOutcome { metadata_revision: plan.metadata_revision, listing_hash: plan.listing_hash, capabilities_hash: plan.capabilities_hash, resend_listing: plan.resend_listing, resend_capabilities: plan.resend_capabilities })
    }

    async fn unpublish(&self, id: &RealmId, now: i64, ts: i64) -> StoreResult<u64> {
        let mut c = self.conn().await?;
        let text = id.to_string();
        let tx = c.transaction().await.map_err(backend)?;
        let r = tx.query_opt("SELECT published, last_request_ts, metadata_revision FROM realms WHERE realm_id = $1::text::uuid FOR UPDATE", &[&text]).await.map_err(backend)?.ok_or(StoreError::Unknown)?;
        let (published, last, revision): (bool, i64, i64) = (r.get(0), r.get(1), r.get(2));
        if ts <= last {
            return Err(StoreError::Stale);
        }
        let revision = revision as u64 + u64::from(published);
        tx.execute(
            "UPDATE realms SET published = false, metadata_revision = $2, last_request_ts = $3, updated_at = CASE WHEN $4 THEN to_timestamp($5::bigint) ELSE updated_at END WHERE realm_id = $1::text::uuid",
            &[&text, &(revision as i64), &ts, &published, &now],
        )
        .await
        .map_err(backend)?;
        tx.commit().await.map_err(backend)?;
        Ok(revision)
    }

    async fn list(&self, q: &ListQuery, now: i64, ttl: i64) -> StoreResult<Vec<RealmSummary>> {
        self.list_rows(q, now, ttl).await
    }
}
