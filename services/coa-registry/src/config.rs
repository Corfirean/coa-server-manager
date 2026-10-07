//! Configuration from the environment. The database password is read from a file (a Docker secret), never from the environment or the image.

use std::env;

use crate::api::ApiConfig;
use crate::limits::Rate;
use crate::pg::PgConfig;

pub struct Config {
    pub listen: String,
    pub max_connections: usize,
    pub request_timeout_secs: u64,
    pub max_in_flight: usize,
    pub api: ApiConfig,
    pub pg: PgConfig,
}

fn var(name: &str, default: &str) -> String {
    env::var(name).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

fn num<T: std::str::FromStr>(name: &str, default: T) -> Result<T, String> {
    match env::var(name).ok().filter(|v| !v.is_empty()) {
        Some(v) => v.parse().map_err(|_| format!("{name} is not a valid number")),
        None => Ok(default),
    }
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let password_file = env::var("REGISTRY_DB_PASSWORD_FILE").map_err(|_| "REGISTRY_DB_PASSWORD_FILE is required".to_string())?;
        let password = std::fs::read_to_string(&password_file).map_err(|e| format!("cannot read the database password file: {e}"))?.trim().to_string();
        if password.is_empty() {
            return Err("the database password file is empty".into());
        }
        let mut api = ApiConfig { trust_proxy: var("REGISTRY_TRUST_PROXY", "0") == "1", ..ApiConfig::default() };
        api.online_ttl_secs = num("REGISTRY_ONLINE_TTL_SECS", api.online_ttl_secs)?;
        api.register = Rate::per_hour(num("REGISTRY_REGISTER_BURST", 10)?, num("REGISTRY_REGISTER_PER_HOUR", 10.0)?);
        api.general = Rate::per_minute(num("REGISTRY_GENERAL_BURST", 120)?, num("REGISTRY_GENERAL_PER_MINUTE", 120.0)?);
        Ok(Self {
            listen: var("REGISTRY_LISTEN", "0.0.0.0:8080"),
            max_connections: num("REGISTRY_MAX_CONNECTIONS", 256)?,
            request_timeout_secs: num("REGISTRY_REQUEST_TIMEOUT_SECS", 10)?,
            max_in_flight: num("REGISTRY_MAX_IN_FLIGHT", 64)?,
            api,
            pg: PgConfig {
                host: var("REGISTRY_DB_HOST", "postgres"),
                port: num("REGISTRY_DB_PORT", 5432)?,
                dbname: var("REGISTRY_DB_NAME", "coa"),
                user: var("REGISTRY_DB_USER", "coa"),
                password,
                pool_size: num("REGISTRY_DB_POOL", 8)?,
                statement_timeout_ms: num("REGISTRY_DB_STATEMENT_TIMEOUT_MS", 5000)?,
            },
        })
    }
}
