use std::sync::Arc;
use std::time::Duration;

use coa_registry::api::{router, AppState, Clock};
use coa_registry::config::Config;
use coa_registry::listener::LimitedListener;
use coa_registry::pg::PgStore;
use tower::limit::ConcurrencyLimitLayer;
use tower_http::timeout::TimeoutLayer;

fn clock() -> Clock {
    Arc::new(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0))
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

/// `coa-registry --healthcheck`: ask the local server for its health and exit 0 or 1 (the image has no curl).
fn healthcheck() -> i32 {
    use std::io::{Read, Write};
    let addr = std::env::var("REGISTRY_LISTEN").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let port = addr.rsplit(':').next().unwrap_or("8080");
    let Ok(mut s) = std::net::TcpStream::connect(format!("127.0.0.1:{port}")) else { return 1 };
    let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    if s.write_all(b"GET /registry/v2/healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").is_err() {
        return 1;
    }
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    i32::from(!out.starts_with("HTTP/1.1 200"))
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("--healthcheck") {
        std::process::exit(healthcheck());
    }
    tracing_subscriber::fmt().json().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).with_current_span(false).init();
    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "configuration");
            std::process::exit(2);
        }
    };
    let store = match PgStore::connect(&cfg.pg).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "database pool");
            std::process::exit(2);
        }
    };
    // the database may still be starting: wait for it instead of crashing in a loop
    let mut waited = 0;
    loop {
        match store.migrate().await {
            Ok(()) => break,
            Err(e) if e.contains("refusing to start") || e.starts_with("migration ") => {
                tracing::error!(error = %e, "migration");
                std::process::exit(3);
            }
            Err(e) => {
                waited += 1;
                if waited > 60 {
                    tracing::error!(error = %e, "the database never became ready");
                    std::process::exit(3);
                }
                tracing::warn!(error = %e, "waiting for the database");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
    let state = Arc::new(AppState::new(store, clock(), cfg.api.clone()));
    let app = router(state).layer(ConcurrencyLimitLayer::new(cfg.max_in_flight)).layer(TimeoutLayer::with_status_code(axum::http::StatusCode::REQUEST_TIMEOUT, Duration::from_secs(cfg.request_timeout_secs)));
    let tcp = match tokio::net::TcpListener::bind(&cfg.listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, listen = %cfg.listen, "bind");
            std::process::exit(2);
        }
    };
    tracing::info!(listen = %cfg.listen, "registry started");
    let listener = LimitedListener::new(tcp, cfg.max_connections);
    let serve = axum::serve(listener, app.into_make_service_with_connect_info::<coa_registry::listener::Peer>()).with_graceful_shutdown(shutdown());
    if let Err(e) = serve.await {
        tracing::error!(error = %e, "server");
        std::process::exit(1);
    }
    tracing::info!("registry stopped");
}
