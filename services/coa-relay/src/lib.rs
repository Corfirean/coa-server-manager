//! Game Relay service library (Phase 13).

pub mod hub;
pub mod limits;
pub mod ports;
pub mod tunnel;

use std::sync::Arc;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use crate::hub::{Hub, KeyLookup};

pub fn router<K: KeyLookup>(hub: Arc<Hub<K>>) -> Router {
    Router::new()
        .route("/relay/v1/health", get(health::<K>))
        .route("/relay/v1/host", get(host_ws::<K>))
        .with_state(hub)
}

async fn health<K: KeyLookup>(State(hub): State<Arc<Hub<K>>>) -> impl IntoResponse {
    let hosts_count = hub.hosts.lock().unwrap().len();
    let allocs_count = hub.allocations.lock().unwrap().len();
    Json(json!({
        "status": "ok",
        "service": "coa-relay",
        "version": coa_control_proto::relay::RELAY_PROTOCOL_VERSION,
        "connected_hosts": hosts_count,
        "active_allocations": allocs_count,
    }))
}

async fn host_ws<K: KeyLookup>(
    ws: WebSocketUpgrade,
    State(hub): State<Arc<Hub<K>>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| tunnel::handle_host_websocket(socket, hub))
}

