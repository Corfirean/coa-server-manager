use std::sync::Arc;
use std::time::Duration;

use coa_relay::hub::{Hub, PgKeys};
use coa_relay::router;
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use tokio_postgres::NoTls;
use tower::limit::ConcurrencyLimitLayer;

fn var(name: &str, default: &str) -> String {
    std::env::var(name).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

fn pool() -> Result<Pool, String> {
    let file = std::env::var("RELAY_DB_PASSWORD_FILE")
        .or_else(|_| std::env::var("COORD_DB_PASSWORD_FILE"))
        .map_err(|_| "RELAY_DB_PASSWORD_FILE or COORD_DB_PASSWORD_FILE is required".to_string())?;
    let password = std::fs::read_to_string(&file)
        .map_err(|e| format!("cannot read database password file: {e}"))?
        .trim()
        .to_string();
    let mut pg = tokio_postgres::Config::new();
    pg.host(&var("RELAY_DB_HOST", &var("COORD_DB_HOST", "coa-postgres")))
        .port(var("RELAY_DB_PORT", &var("COORD_DB_PORT", "5432")).parse().map_err(|_| "RELAY_DB_PORT")?)
        .dbname(&var("RELAY_DB_NAME", &var("COORD_DB_NAME", "coa_registry")))
        .user(&var("RELAY_DB_USER", &var("COORD_DB_USER", "coa_coordinator")))
        .password(&password);
    pg.connect_timeout(Duration::from_secs(5))
        .application_name("coa-relay")
        .options("-c statement_timeout=3000 -c default_transaction_read_only=on");
    Pool::builder(Manager::from_config(pg, NoTls, ManagerConfig { recycling_method: RecyclingMethod::Fast }))
        .max_size(4)
        .wait_timeout(Some(Duration::from_secs(5)))
        .create_timeout(Some(Duration::from_secs(5)))
        .runtime(Runtime::Tokio1)
        .build()
        .map_err(|e| e.to_string())
}

fn healthcheck() -> i32 {
    use std::io::{Read, Write};
    let port = var("RELAY_LISTEN", "0.0.0.0:8082").rsplit(':').next().unwrap_or("8082").to_string();
    let Ok(mut s) = std::net::TcpStream::connect(format!("127.0.0.1:{port}")) else { return 1 };
    let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    if s.write_all(b"GET /relay/v1/health HTTP/1.1\r\nHost: localhost\r\n\r\n").is_err() {
        return 1;
    }
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

    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_current_span(false)
        .init();

    let pool = match pool() {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "database configuration error");
            std::process::exit(2);
        }
    };

    let relay_host = var("RELAY_HOST", "coa-manager.duckdns.org");
    let min_port: u16 = var("RELAY_PORT_MIN", "40000").parse().expect("valid RELAY_PORT_MIN");
    let max_port: u16 = var("RELAY_PORT_MAX", "40050").parse().expect("valid RELAY_PORT_MAX");

    let clock = Arc::new(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0));
    let hub = Arc::new(Hub::new(relay_host, min_port, max_port, PgKeys::new(pool), clock));

    let app = router(hub).layer(ConcurrencyLimitLayer::new(512));
    let listen = var("RELAY_LISTEN", "0.0.0.0:8082");

    let tcp = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(listen = %listen, error = %e, "cannot bind the listening socket");
            std::process::exit(3);
        }
    };

    tracing::info!(listen = %listen, "coa-relay listening");
    if let Err(e) = axum::serve(tcp, app.into_make_service()).with_graceful_shutdown(shutdown()).await {
        tracing::error!(error = %e, "service crashed");
        std::process::exit(1);
    }
}
