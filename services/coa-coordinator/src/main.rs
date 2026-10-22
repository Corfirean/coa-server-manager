use std::sync::Arc;
use std::time::Duration;

use coa_coordinator::listener::{LimitedListener, Peer};
use coa_coordinator::{router, Config, Hub, PgKeys};
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use tokio_postgres::NoTls;
use tower::limit::ConcurrencyLimitLayer;

fn var(name: &str, default: &str) -> String {
    std::env::var(name).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

fn pool() -> Result<Pool, String> {
    let file = std::env::var("COORD_DB_PASSWORD_FILE").map_err(|_| "COORD_DB_PASSWORD_FILE is required".to_string())?;
    let password = std::fs::read_to_string(&file).map_err(|e| format!("cannot read the database password file: {e}"))?.trim().to_string();
    let mut pg = tokio_postgres::Config::new();
    pg.host(&var("COORD_DB_HOST", "coa-postgres")).port(var("COORD_DB_PORT", "5432").parse().map_err(|_| "COORD_DB_PORT")?).dbname(&var("COORD_DB_NAME", "coa_registry")).user(&var("COORD_DB_USER", "coa_coordinator")).password(&password);
    pg.connect_timeout(Duration::from_secs(5)).application_name("coa-coordinator").options("-c statement_timeout=3000 -c default_transaction_read_only=on");
    Pool::builder(Manager::from_config(pg, NoTls, ManagerConfig { recycling_method: RecyclingMethod::Fast })).max_size(4).wait_timeout(Some(Duration::from_secs(5))).create_timeout(Some(Duration::from_secs(5))).runtime(Runtime::Tokio1).build().map_err(|e| e.to_string())
}

/// `coa-coordinator --healthcheck`: ask the local server and exit 0 or 1 (the image has no curl).
fn healthcheck() -> i32 {
    use std::io::{Read, Write};
    let port = var("COORD_LISTEN", "0.0.0.0:8081").rsplit(':').next().unwrap_or("8081").to_string();
    let Ok(mut s) = std::net::TcpStream::connect(format!("127.0.0.1:{port}")) else { return 1 };
    let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    if s.write_all(b"GET /coord/v1/health HTTP/1.1\r\nHost: localhost\r\n\r\n").is_err() {
        return 1;
    }
    // the status line is all that is needed
    let mut buf = [0u8; 16];
    i32::from(!matches!(s.read(&mut buf), Ok(n) if n >= 12 && buf.starts_with(b"HTTP/1.1 200")))
}

async fn shutdown() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("a SIGTERM handler");
        tokio::select! { _ = ctrl_c => {}, _ = term.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("--healthcheck") {
        std::process::exit(healthcheck());
    }
    tracing_subscriber::fmt().json().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).with_current_span(false).init();
    let pool = match pool() {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "configuration");
            std::process::exit(2);
        }
    };
    let cfg = Config { trust_proxy: var("COORD_TRUST_PROXY", "0") == "1", ..Config::default() };
    let clock = Arc::new(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0));
    let hub = Hub::new(PgKeys::new(pool), cfg, clock);
    let app = router(hub).layer(ConcurrencyLimitLayer::new(512));
    let listen = var("COORD_LISTEN", "0.0.0.0:8081");
    let tcp = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, %listen, "bind");
            std::process::exit(2);
        }
    };
    tracing::info!(%listen, "coordinator started");
    let serve = axum::serve(LimitedListener::new(tcp, 512), app.into_make_service_with_connect_info::<Peer>()).with_graceful_shutdown(shutdown());
    if let Err(e) = serve.await {
        tracing::error!(error = %e, "server");
        std::process::exit(1);
    }
}
