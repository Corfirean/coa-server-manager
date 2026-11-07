//! Public AzerothCore Module Catalogue integration for CoA Server Manager.
//!
//! Provides cached, prioritized, and overlay-enriched discovery of AzerothCore modules
//! sourced from `https://www.azerothcore.org/data/catalogue.json`.
//!
//! Features:
//! - Thread-safe periodic synchronization with ETag (`If-None-Match`) support
//! - Transparent offline caching with atomic disk persistence
//! - Priority ordering: CoA Recommended / Verified modules always appear first
//! - Seamless deduplication by GitHub Repository ID and normalized slug
//! - Deep integration with the CoA Compatibility Overlay (`coa-modules-overlay.json`)
//! - Hard policy blocking to prevent installation of known crashers or security risks

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fsx;

pub const OFFICIAL_CATALOGUE_URL: &str = "https://www.azerothcore.org/data/catalogue.json";
pub const DEFAULT_CACHE_TTL_SECS: i64 = 12 * 3600; // 12 hours
const CONNECT_TIMEOUT_SECS: u64 = 10;
const READ_TIMEOUT_SECS: u64 = 25;

static CATALOGUE_LOCK: Mutex<()> = Mutex::new(());
static EMBEDDED_OVERLAY_JSON: &str = include_str!("coa-modules-overlay.json");

// ---------------------------------------------------------------------------
// External Data Models (matching official AzerothCore JSON)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct OfficialCatalogueRaw {
    #[serde(default)]
    pub global_search: bool,
    #[serde(default)]
    pub organizations: BTreeMap<String, BTreeMap<String, Vec<OfficialRepoRaw>>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OfficialRepoRaw {
    pub id: u64,
    pub name: String,
    pub full_name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub stargazers_count: u32,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default = "default_branch_val")]
    pub default_branch: String,
    #[serde(default)]
    pub owner: Option<OfficialOwnerRaw>,
    pub html_url: String,
    #[serde(default)]
    pub forks_count: u32,
    #[serde(default)]
    pub watchers_count: u32,
    #[serde(default)]
    pub pushed_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub license: Option<OfficialLicenseRaw>,
    #[serde(default)]
    pub topics: Vec<String>,
}

fn default_branch_val() -> String {
    "master".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct OfficialOwnerRaw {
    pub login: String,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OfficialLicenseRaw {
    #[serde(default)]
    pub spdx_id: Option<String>,
}

// ---------------------------------------------------------------------------
// CoA Overlay Models
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoaOverlayDocument {
    pub version: u32,
    #[serde(default)]
    pub description: Option<String>,
    pub rules: Vec<CoaOverlayRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoaOverlayRule {
    #[serde(default)]
    pub repo_id: Option<u64>,
    pub repo_slug: String,
    pub status: CoaCompatibilityStatus,
    #[serde(default)]
    pub is_recommended: bool,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub tested_version: Option<String>,
    #[serde(default)]
    pub tested_abi: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CoaCompatibilityStatus {
    Recommended,
    Verified,
    RequiresAdapter,
    Unverified,
    Incompatible,
    Blocked,
}

impl std::fmt::Display for CoaCompatibilityStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Recommended => write!(f, "Recommended"),
            Self::Verified => write!(f, "Verified"),
            Self::RequiresAdapter => write!(f, "RequiresAdapter"),
            Self::Unverified => write!(f, "Unverified"),
            Self::Incompatible => write!(f, "Incompatible"),
            Self::Blocked => write!(f, "Blocked"),
        }
    }
}

// ---------------------------------------------------------------------------
// Internal Normalized Item & API Response
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicCatalogueItem {
    pub id: u64,
    pub name: String,
    pub full_name: String,
    pub owner: String,
    pub html_url: String,
    pub description: String,
    pub stars: u32,
    pub forks: u32,
    pub default_branch: String,
    #[serde(default)]
    pub pushed_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub topics: Vec<String>,
    pub coa_status: CoaCompatibilityStatus,
    pub coa_status_source: String, // "Overlay", "VerifiedCatalog", "AutoPublic"
    pub conflict_reason: Option<String>,
    pub is_coa_recommended: bool,
    /// Functional category code (see `coa-module-categories.json`); "other" when nothing fits.
    #[serde(default)]
    pub category: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryInfo {
    pub code: String,
    pub ru: String,
    pub en: String,
    #[serde(default)]
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CategoryDocument {
    categories: Vec<CategoryInfo>,
    /// Owner-reviewed assignments (lowercase `owner/repo` -> category code).
    repos: BTreeMap<String, String>,
}

static EMBEDDED_CATEGORIES_JSON: &str = include_str!("coa-module-categories.json");

fn category_document() -> &'static CategoryDocument {
    static DOC: std::sync::OnceLock<CategoryDocument> = std::sync::OnceLock::new();
    DOC.get_or_init(|| {
        serde_json::from_str(EMBEDDED_CATEGORIES_JSON).unwrap_or(CategoryDocument { categories: Vec::new(), repos: BTreeMap::new() })
    })
}

pub fn categories() -> Vec<CategoryInfo> {
    category_document().categories.clone()
}

/// Lowercase words of a text: letters and digits, so `mod-pvp-titles` gives `mod`, `pvp`, `titles`.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '+')
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

fn keyword_hits(keyword: &str, text_words: &[String]) -> usize {
    let kw = words(keyword);
    match kw.as_slice() {
        [] => 0,
        // one word: whole-word match; words of 4+ letters also match their plural or longer forms
        [k] => text_words.iter().filter(|w| *w == k || (k.chars().count() >= 4 && w.starts_with(k.as_str()))).count(),
        // a phrase: its words in a row
        many => text_words.windows(many.len()).filter(|win| win.iter().zip(many).all(|(a, b)| a == b)).count(),
    }
}

/// The category of a module: the owner-reviewed list first, then the keywords of each category against the
/// repository's name (counted twice), description and topics; "other" when no keyword matches.
pub fn categorize(slug: &str, name: &str, description: &str, topics: &[String]) -> String {
    let doc = category_document();
    if let Some(code) = doc.repos.get(&normalize_repo_slug(slug)) {
        return code.clone();
    }
    let name_words = words(name);
    let mut body = words(description);
    for t in topics.iter().filter(|t| !t.starts_with("azerothcore") && !matches!(t.as_str(), "wow" | "wotlk" | "world-of-warcraft")) {
        body.extend(words(t));
    }
    let mut best: Option<(&str, usize)> = None;
    for c in &doc.categories {
        let score: usize = c.keywords.iter().map(|k| 2 * keyword_hits(k, &name_words) + keyword_hits(k, &body)).sum();
        if score > 0 && best.is_none_or(|(_, s)| score > s) {
            best = Some((&c.code, score));
        }
    }
    best.map(|(c, _)| c.to_string()).unwrap_or_else(|| "other".into())
}

/// Statuses and categories come from the current overlay and category list on every read, never from what a
/// cache stored: a corrected overlay must show at once. Entries an earlier version made up for overlay rules the
/// public catalogue does not contain (marked with the `coa-verified` topic) are dropped.
fn finalize(items: Vec<PublicCatalogueItem>, overlay: &CoaOverlayDocument) -> Vec<PublicCatalogueItem> {
    let rules: BTreeMap<String, &CoaOverlayRule> =
        overlay.rules.iter().map(|r| (normalize_repo_slug(&r.repo_slug), r)).collect();
    items
        .into_iter()
        .filter(|i| !i.topics.iter().any(|t| t == "coa-verified"))
        .map(|mut i| {
            let slug = normalize_repo_slug(&i.full_name);
            match rules.get(&slug) {
                Some(rule) => {
                    i.coa_status = rule.status;
                    i.coa_status_source = "Overlay".into();
                    i.conflict_reason = rule.reason.clone();
                    i.is_coa_recommended = rule.is_recommended || rule.status == CoaCompatibilityStatus::Recommended;
                }
                None => {
                    i.coa_status = CoaCompatibilityStatus::Unverified;
                    i.coa_status_source = "AutoPublic".into();
                    i.conflict_reason = None;
                    i.is_coa_recommended = false;
                }
            }
            i.category = categorize(&slug, &i.name, &i.description, &i.topics);
            i
        })
        .collect()
}

fn respond(items: Vec<PublicCatalogueItem>, overlay: &CoaOverlayDocument, cached_at: String, is_offline: bool, etag: Option<String>) -> PublicCatalogueResponse {
    let items = finalize(items, overlay);
    PublicCatalogueResponse {
        total_count: items.len(),
        recommended_count: items.iter().filter(|i| i.is_coa_recommended).count(),
        cached_at,
        is_offline,
        etag,
        items,
        categories: categories().into_iter().map(|c| CategoryInfo { keywords: Vec::new(), ..c }).collect(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedDocument {
    pub schema_version: u32,
    pub cached_at: String,
    pub timestamp: i64,
    #[serde(default)]
    pub etag: Option<String>,
    pub items: Vec<PublicCatalogueItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicCatalogueResponse {
    pub items: Vec<PublicCatalogueItem>,
    pub cached_at: String,
    pub is_offline: bool,
    #[serde(default)]
    pub etag: Option<String>,
    pub total_count: usize,
    pub recommended_count: usize,
    #[serde(default)]
    pub categories: Vec<CategoryInfo>,
}

// ---------------------------------------------------------------------------
// Helper: Normalization of Slugs & URLs
// ---------------------------------------------------------------------------

pub fn normalize_repo_slug(url_or_slug: &str) -> String {
    let mut s = url_or_slug.trim();
    if let Some(pos) = s.find("://") {
        s = &s[pos + 3..];
    }
    if let Some(pos) = s.find('@') {
        if let Some(colon) = s[pos..].find(':') {
            s = &s[pos + colon + 1..];
        }
    }
    if let Some(stripped) = s.strip_prefix("github.com/") {
        s = stripped;
    }
    // Remove query params or fragments
    if let Some(pos) = s.find('?') {
        s = &s[..pos];
    }
    if let Some(pos) = s.find('#') {
        s = &s[..pos];
    }
    // Remove tree/branches if present (e.g. repo/tree/branch/sub)
    let parts: Vec<&str> = s.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() >= 2 {
        let owner = parts[0].to_ascii_lowercase();
        let mut repo = parts[1].to_ascii_lowercase();
        if repo.ends_with(".git") {
            repo.truncate(repo.len() - 4);
        }
        format!("{owner}/{repo}")
    } else {
        s.trim_end_matches(".git").to_ascii_lowercase()
    }
}

pub fn load_coa_overlay(optional_overlay_path: Option<&Path>) -> CoaOverlayDocument {
    let mut doc: CoaOverlayDocument = serde_json::from_str(EMBEDDED_OVERLAY_JSON)
        .unwrap_or_else(|_| CoaOverlayDocument {
            version: 1,
            description: None,
            rules: Vec::new(),
        });

    if let Some(path) = optional_overlay_path {
        if path.is_file() {
            if let Ok(content) = fs::read_to_string(path) {
                if let Ok(external) = serde_json::from_str::<CoaOverlayDocument>(&content) {
                    for ext_rule in external.rules {
                        if let Some(existing) = doc
                            .rules
                            .iter_mut()
                            .find(|r| normalize_repo_slug(&r.repo_slug) == normalize_repo_slug(&ext_rule.repo_slug))
                        {
                            *existing = ext_rule;
                        } else {
                            doc.rules.push(ext_rule);
                        }
                    }
                }
            }
        }
    }

    doc
}

/// Checks if a repository is explicitly blocked by CoA policy.
/// Returns `Some(reason)` if blocked, otherwise `None`.
pub fn is_blocked_repo(url_or_slug: &str) -> Option<String> {
    let target = normalize_repo_slug(url_or_slug);
    let overlay = load_coa_overlay(None);
    for rule in overlay.rules {
        if rule.status == CoaCompatibilityStatus::Blocked && normalize_repo_slug(&rule.repo_slug) == target {
            return Some(rule.reason.unwrap_or_else(|| "Module blocked by CoA stability/security policy.".into()));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Normalization & Deduplication Logic
// ---------------------------------------------------------------------------

pub fn normalize_catalogue(
    raw: &OfficialCatalogueRaw,
    overlay: &CoaOverlayDocument,
) -> Vec<PublicCatalogueItem> {
    let mut normalized = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut seen_slugs = HashSet::new();

    // 1. Map overlay rules by slug for O(1) lookup
    let mut overlay_map: BTreeMap<String, &CoaOverlayRule> = BTreeMap::new();
    for rule in &overlay.rules {
        overlay_map.insert(normalize_repo_slug(&rule.repo_slug), rule);
    }

    // 2. Extract repos from the category "azerothcore-module"
    let mut repos = Vec::new();
    for (_, topics) in &raw.organizations {
        for (topic_name, list) in topics {
            if topic_name == "azerothcore-module" {
                repos.extend(list.iter());
            }
        }
    }

    // If "azerothcore-module" was empty or structure differs, collect all modules
    if repos.is_empty() {
        for (_, topics) in &raw.organizations {
            for (_, list) in topics {
                repos.extend(list.iter());
            }
        }
    }

    // 3. Process and deduplicate
    for repo in repos {
        let slug = normalize_repo_slug(&repo.full_name);

        if repo.id != 0 && !seen_ids.insert(repo.id) {
            continue;
        }
        if !seen_slugs.insert(slug.clone()) {
            continue;
        }

        let owner = repo
            .owner
            .as_ref()
            .map(|o| o.login.clone())
            .unwrap_or_else(|| {
                slug.split('/').next().unwrap_or("unknown").to_string()
            });

        // Determine CoA status from overlay or default to Unverified
        let (coa_status, coa_source, reason, is_rec) = if let Some(rule) = overlay_map.get(&slug) {
            (
                rule.status,
                "Overlay".to_string(),
                rule.reason.clone(),
                rule.is_recommended || rule.status == CoaCompatibilityStatus::Recommended,
            )
        } else {
            (
                CoaCompatibilityStatus::Unverified,
                "AutoPublic".to_string(),
                None,
                false,
            )
        };

        let license = repo.license.as_ref().and_then(|l| l.spdx_id.clone());

        normalized.push(PublicCatalogueItem {
            id: repo.id,
            name: repo.name.clone(),
            full_name: repo.full_name.clone(),
            owner,
            html_url: repo.html_url.clone(),
            description: repo.description.clone().unwrap_or_default(),
            stars: repo.stargazers_count,
            forks: repo.forks_count,
            default_branch: repo.default_branch.clone(),
            pushed_at: repo.pushed_at.clone(),
            updated_at: repo.updated_at.clone(),
            license,
            topics: repo.topics.clone(),
            coa_status,
            coa_status_source: coa_source,
            conflict_reason: reason,
            is_coa_recommended: is_rec,
            category: String::new(),
        });
    }

    // Overlay rules for repositories the public catalogue does not list are not turned into entries: the
    // catalogue shows only what azerothcore.org lists (CoA's own modules are in the Manager's curated list).

    // 4. Sort:
    // Group A (Recommended/Verified) first, then by Stars desc, then by pushed_at desc
    normalized.sort_by(|a, b| {
        // Priority 1: Recommended modules first
        match (a.is_coa_recommended, b.is_coa_recommended) {
            (true, false) => return std::cmp::Ordering::Less,
            (false, true) => return std::cmp::Ordering::Greater,
            _ => {}
        }
        // Priority 2: Status order (Verified before Unverified, Blocked at end)
        if a.coa_status != b.coa_status {
            return a.coa_status.cmp(&b.coa_status);
        }
        // Priority 3: Stars descending
        if b.stars != a.stars {
            return b.stars.cmp(&a.stars);
        }
        // Priority 4: pushed_at descending
        let pa = a.pushed_at.as_deref().unwrap_or("");
        let pb = b.pushed_at.as_deref().unwrap_or("");
        if pb != pa {
            return pb.cmp(pa);
        }
        // Deterministic tie-breaker
        a.full_name.cmp(&b.full_name)
    });

    normalized
}

// ---------------------------------------------------------------------------
// Network Fetcher & Cache Manager
// ---------------------------------------------------------------------------

pub fn fetch_or_load_catalogue(
    cache_dir: &Path,
    force_refresh: bool,
) -> Result<PublicCatalogueResponse> {
    let _guard = CATALOGUE_LOCK.lock().map_err(|_| {
        Error::Invalid("Failed to acquire catalogue cache lock.".into())
    })?;

    fs::create_dir_all(cache_dir)?;
    let cache_file = cache_dir.join("catalogue_cache.json");
    let etag_file = cache_dir.join("catalogue_etag.txt");
    let overlay_path = cache_dir.join("coa-modules-overlay.json");
    let overlay = load_coa_overlay(Some(&overlay_path));

    let now_ts = Utc::now().timestamp();

    // 1. Check if disk cache exists and is fresh
    if !force_refresh && cache_file.is_file() {
        if let Ok(content) = fs::read_to_string(&cache_file) {
            if let Ok(cached) = serde_json::from_str::<CachedDocument>(&content) {
                let age = now_ts - cached.timestamp;
                if age < DEFAULT_CACHE_TTL_SECS && !cached.items.is_empty() {
                    return Ok(respond(cached.items, &overlay, cached.cached_at, false, cached.etag));
                }
            }
        }
    }

    // 2. Perform Network Fetch with ETag support
    let stored_etag = fs::read_to_string(&etag_file).ok().map(|s| s.trim().to_string());
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .timeout(Duration::from_secs(READ_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| Error::Invalid(format!("HTTP client initialization failed: {}", e)))?;

    let mut request = client.get(OFFICIAL_CATALOGUE_URL)
        .header(reqwest::header::USER_AGENT, "CSM-Modules-Preview/0.7.1");

    if let Some(ref etag) = stored_etag {
        if !force_refresh && cache_file.is_file() {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag.as_str());
        }
    }

    let fetch_result = request.send();

    match fetch_result {
        Ok(response) => {
            let status = response.status();

            // 2.1 Handle 304 Not Modified
            if status == reqwest::StatusCode::NOT_MODIFIED {
                if cache_file.is_file() {
                    if let Ok(content) = fs::read_to_string(&cache_file) {
                        if let Ok(mut cached) = serde_json::from_str::<CachedDocument>(&content) {
                            cached.timestamp = now_ts;
                            cached.cached_at = Utc::now().to_rfc3339();
                            let _ = fsx::atomic_write_json(&cache_file, &cached);
                            return Ok(respond(cached.items, &overlay, cached.cached_at, false, cached.etag));
                        }
                    }
                }
                // If 304 received but cache file was missing or corrupt, re-fetch without etag
                return fetch_or_load_catalogue(cache_dir, true);
            }

            // 2.2 Handle 200 OK
            if status.is_success() {
                let new_etag = response
                    .headers()
                    .get(reqwest::header::ETAG)
                    .and_then(|h| h.to_str().ok())
                    .map(|s| s.to_string());

                let text = response.text().map_err(|e| {
                    Error::Invalid(format!("Failed to read catalogue response body: {}", e))
                })?;

                let raw: OfficialCatalogueRaw = serde_json::from_str(&text).map_err(|e| {
                    Error::Invalid(format!("Failed to parse AzerothCore official catalogue JSON: {}", e))
                })?;

                let items = normalize_catalogue(&raw, &overlay);
                let now_str = Utc::now().to_rfc3339();

                let cached_doc = CachedDocument {
                    schema_version: 1,
                    cached_at: now_str.clone(),
                    timestamp: now_ts,
                    etag: new_etag.clone(),
                    items: items.clone(),
                };

                // Persist cache and etag atomically
                let _ = fsx::atomic_write_json(&cache_file, &cached_doc);
                if let Some(ref tag) = new_etag {
                    let _ = fs::write(&etag_file, tag);
                }

                return Ok(respond(items, &overlay, now_str, false, new_etag));
            }

            // If non-200 and non-304 HTTP status, fallback to existing cache if available
            try_offline_fallback(&cache_file, stored_etag, &overlay)
        }
        Err(_net_err) => {
            // Network unreachable, DNS timeout, connection reset -> offline fallback
            try_offline_fallback(&cache_file, stored_etag, &overlay)
        }
    }
}

fn try_offline_fallback(
    cache_file: &Path,
    stored_etag: Option<String>,
    overlay: &CoaOverlayDocument,
) -> Result<PublicCatalogueResponse> {
    if cache_file.is_file() {
        if let Ok(content) = fs::read_to_string(cache_file) {
            if let Ok(cached) = serde_json::from_str::<CachedDocument>(&content) {
                return Ok(respond(cached.items, overlay, cached.cached_at, true, cached.etag.or(stored_etag)));
            }
        }
    }

    // If completely offline and no cache file exists, build minimal catalog from overlay rules
    let empty_raw = OfficialCatalogueRaw {
        global_search: false,
        organizations: BTreeMap::new(),
    };
    let items = normalize_catalogue(&empty_raw, overlay);
    Ok(respond(items, overlay, Utc::now().to_rfc3339(), true, None))
}

// ---------------------------------------------------------------------------
// Unit & Integration Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_official_json() -> &'static str {
        r#"{
            "global_search": true,
            "organizations": {
                "azerothcore": {
                    "azerothcore-module": [
                        {
                            "id": 101,
                            "name": "mod-npc-buffer",
                            "full_name": "azerothcore/mod-npc-buffer",
                            "description": "NPC buffer master",
                            "stargazers_count": 55,
                            "default_branch": "master",
                            "html_url": "https://github.com/azerothcore/mod-npc-buffer",
                            "forks_count": 12,
                            "watchers_count": 55,
                            "pushed_at": "2026-10-10T12:00:00Z",
                            "updated_at": "2026-10-10T12:00:00Z",
                            "license": { "spdx_id": "AGPL-3.0" },
                            "topics": ["azerothcore-module", "utility"]
                        },
                        {
                            "id": 102,
                            "name": "mod-random-community",
                            "full_name": "author/mod-random-community",
                            "description": "A community module",
                            "stargazers_count": 120,
                            "default_branch": "main",
                            "html_url": "https://github.com/author/mod-random-community",
                            "forks_count": 5,
                            "watchers_count": 120,
                            "pushed_at": "2026-09-01T12:00:00Z",
                            "updated_at": "2026-09-01T12:00:00Z",
                            "license": null,
                            "topics": ["azerothcore-module"]
                        },
                        {
                            "id": 103,
                            "name": "mod-learn-spells",
                            "full_name": "azerothcore/mod-learn-spells",
                            "description": "Auto teach spells",
                            "stargazers_count": 48,
                            "default_branch": "master",
                            "html_url": "https://github.com/azerothcore/mod-learn-spells",
                            "forks_count": 8,
                            "watchers_count": 48,
                            "pushed_at": "2026-08-01T12:00:00Z",
                            "updated_at": "2026-08-01T12:00:00Z",
                            "license": { "spdx_id": "AGPL-3.0" },
                            "topics": ["azerothcore-module"]
                        },
                        {
                            "id": 104,
                            "name": "mod-ahbot",
                            "full_name": "azerothcore/mod-ahbot",
                            "description": "Upstream ahbot",
                            "stargazers_count": 80,
                            "default_branch": "master",
                            "html_url": "https://github.com/azerothcore/mod-ahbot",
                            "forks_count": 20,
                            "watchers_count": 80,
                            "pushed_at": "2026-07-01T12:00:00Z",
                            "updated_at": "2026-07-01T12:00:00Z",
                            "license": null,
                            "topics": ["azerothcore-module"]
                        },
                        {
                            "id": 101,
                            "name": "mod-npc-buffer-duplicate",
                            "full_name": "azerothcore/mod-npc-buffer",
                            "description": "Duplicate entry with same id and slug",
                            "stargazers_count": 55,
                            "default_branch": "master",
                            "html_url": "https://github.com/azerothcore/mod-npc-buffer",
                            "forks_count": 12,
                            "watchers_count": 55,
                            "pushed_at": "2026-10-10T12:00:00Z",
                            "updated_at": "2026-10-10T12:00:00Z",
                            "license": { "spdx_id": "AGPL-3.0" },
                            "topics": ["azerothcore-module"]
                        }
                    ],
                    "azerothcore-lua": [
                        {
                            "id": 201,
                            "name": "pure-lua-script",
                            "full_name": "some/pure-lua-script",
                            "stargazers_count": 10,
                            "default_branch": "main",
                            "html_url": "https://github.com/some/pure-lua-script"
                        }
                    ]
                }
            }
        }"#
    }

    #[test]
    fn test_slug_normalization() {
        assert_eq!(normalize_repo_slug("https://github.com/azerothcore/mod-npc-buffer.git"), "azerothcore/mod-npc-buffer");
        assert_eq!(normalize_repo_slug("https://github.com/AzerothCore/Mod-NPC-Buffer/"), "azerothcore/mod-npc-buffer");
        assert_eq!(normalize_repo_slug("git@github.com:Corfirean/mod-coa-playerbots.git"), "corfirean/mod-coa-playerbots");
        assert_eq!(normalize_repo_slug("author/repo"), "author/repo");
    }

    #[test]
    fn test_overlay_loading_and_blocked_detection() {
        let overlay = load_coa_overlay(None);
        assert!(!overlay.rules.is_empty(), "Embedded overlay must contain rules");

        // Verify mod-ahbot is blocked
        assert!(is_blocked_repo("https://github.com/azerothcore/mod-ahbot").is_some());
        assert!(is_blocked_repo("azerothcore/mod-ahbot").is_some());

        // Verify clean repo is not blocked
        assert!(is_blocked_repo("https://github.com/azerothcore/mod-npc-buffer").is_none());
        assert!(is_blocked_repo("random-author/clean-module").is_none());
    }

    #[test]
    fn test_official_json_parsing_and_normalization() {
        let raw: OfficialCatalogueRaw = serde_json::from_str(sample_official_json()).expect("Must parse sample JSON");
        let overlay = load_coa_overlay(None);
        let items = normalize_catalogue(&raw, &overlay);

        // Deduplication: id 101 was duplicated, should only appear once
        let buffer_count = items.iter().filter(|i| i.full_name == "azerothcore/mod-npc-buffer").count();
        assert_eq!(buffer_count, 1, "Duplicate repo with ID 101 must be deduplicated");

        // Check statuses assigned via overlay
        let buffer = items.iter().find(|i| i.full_name == "azerothcore/mod-npc-buffer").expect("mod-npc-buffer must exist");
        assert_eq!(buffer.coa_status, CoaCompatibilityStatus::Verified);
        assert!(buffer.is_coa_recommended);

        let learn_spells = items.iter().find(|i| i.full_name == "azerothcore/mod-learn-spells").expect("mod-learn-spells must exist");
        assert_eq!(learn_spells.coa_status, CoaCompatibilityStatus::Incompatible);
        assert!(!learn_spells.is_coa_recommended);
        assert!(learn_spells.conflict_reason.is_some());

        let ahbot = items.iter().find(|i| i.full_name == "azerothcore/mod-ahbot").expect("mod-ahbot must exist");
        assert_eq!(ahbot.coa_status, CoaCompatibilityStatus::Blocked);

        let community = items.iter().find(|i| i.full_name == "author/mod-random-community").expect("community module must exist");
        assert_eq!(community.coa_status, CoaCompatibilityStatus::Unverified);
        assert_eq!(community.coa_status_source, "AutoPublic");
        assert!(!community.is_coa_recommended);

        // Check priority sorting: Recommended modules must come first
        let first_non_rec_idx = items.iter().position(|i| !i.is_coa_recommended).unwrap_or(items.len());
        for (idx, item) in items.iter().enumerate() {
            if idx < first_non_rec_idx {
                assert!(item.is_coa_recommended, "Items before boundary must be recommended");
            } else {
                assert!(!item.is_coa_recommended, "Items after boundary must not be recommended");
            }
        }
    }

    #[test]
    fn test_disk_caching_and_offline_fallback() {
        let d = tempdir().expect("tempdir");
        let cache_dir = d.path();

        // 1. Manually write a cached document
        let items = vec![
            PublicCatalogueItem {
                id: 999,
                name: "cached-mod".into(),
                full_name: "test/cached-mod".into(),
                owner: "test".into(),
                html_url: "https://github.com/test/cached-mod".into(),
                description: "Test description".into(),
                stars: 42,
                forks: 2,
                default_branch: "master".into(),
                pushed_at: Some("2026-10-01T00:00:00Z".into()),
                updated_at: Some("2026-10-01T00:00:00Z".into()),
                license: Some("MIT".into()),
                topics: vec!["azerothcore-module".into()],
                coa_status: CoaCompatibilityStatus::Unverified,
                coa_status_source: "CacheTest".into(),
                conflict_reason: None,
                is_coa_recommended: false,
                category: String::new(),
            }
        ];

        let cached_doc = CachedDocument {
            schema_version: 1,
            cached_at: Utc::now().to_rfc3339(),
            timestamp: Utc::now().timestamp(),
            etag: Some("\"sample-etag\"".into()),
            items: items.clone(),
        };

        let cache_file = cache_dir.join("catalogue_cache.json");
        fsx::atomic_write_json(&cache_file, &cached_doc).expect("write cache");

        // 2. Read without force refresh -> should return cached doc instantly without network
        let resp = fetch_or_load_catalogue(cache_dir, false).expect("load from cache");
        assert_eq!(resp.total_count, 1);
        assert_eq!(resp.items[0].name, "cached-mod");
        assert!(!resp.is_offline);

        // 3. Corrupt the cache file -> test resilience
        fs::write(&cache_file, b"NOT_A_VALID_JSON").expect("write corrupt cache");
        let overlay = load_coa_overlay(None);
        let fallback_resp = try_offline_fallback(&cache_file, None, &overlay).expect("fallback must succeed");
        assert!(fallback_resp.is_offline);
        // nothing is made up from overlay rules when there is no catalogue to show
        assert!(fallback_resp.items.is_empty());
    }

    fn item(full_name: &str, name: &str, description: &str, topics: &[&str]) -> PublicCatalogueItem {
        PublicCatalogueItem {
            id: 1,
            name: name.into(),
            full_name: full_name.into(),
            owner: full_name.split('/').next().unwrap().into(),
            html_url: format!("https://github.com/{full_name}"),
            description: description.into(),
            stars: 0,
            forks: 0,
            default_branch: "master".into(),
            pushed_at: None,
            updated_at: None,
            license: None,
            topics: topics.iter().map(|t| t.to_string()).collect(),
            coa_status: CoaCompatibilityStatus::Verified,
            coa_status_source: "Overlay".into(),
            conflict_reason: Some("stale".into()),
            is_coa_recommended: true,
            category: String::new(),
        }
    }

    #[test]
    fn statuses_come_from_the_current_overlay_not_the_cache_and_made_up_entries_are_dropped() {
        let overlay = load_coa_overlay(None);
        let cached = vec![
            item("azerothcore/mod-duel-reset", "mod-duel-reset", "Duel reset module", &["azerothcore-module"]),
            item("azerothcore/mod-cfbg", "mod-cfbg", "Cross-faction battlegrounds", &["azerothcore-module", "coa-verified"]),
        ];
        let out = finalize(cached, &overlay);
        assert_eq!(out.len(), 1, "the made-up mod-cfbg entry is dropped");
        assert_eq!(out[0].coa_status, CoaCompatibilityStatus::Unverified);
        assert!(out[0].conflict_reason.is_none() && !out[0].is_coa_recommended);
        assert_eq!(out[0].category, "pvp");
    }

    #[test]
    fn modules_get_a_category_from_the_reviewed_list_or_keywords() {
        // reviewed list (the owner's file)
        assert_eq!(categorize("DustinHendrickson/mod-ollama-chat", "mod-ollama-chat", "", &[]), "bots");
        // keywords: whole words, not parts of other words
        assert_eq!(categorize("x/mod-arena-points", "mod-arena-points", "Gives arena points", &[]), "pvp");
        assert_eq!(categorize("x/mod-guild-chat", "mod-guild-chat", "", &[]), "social");
        assert_eq!(categorize("x/mod-maintain", "mod-maintain", "Maintains things", &[]), "other");
        assert_eq!(categorize("x/mod-nothing", "mod-nothing", "", &["azerothcore-module".to_string(), "wotlk".to_string()]), "other");
        assert!(categories().iter().any(|c| c.code == "other"));
    }
}
