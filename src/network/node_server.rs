use crate::{
    cluster::gossip::GossipMessage,
    network::{
        node_client::{DeleteRequest, GetRequest, GetResponse, OperationResponse, SetRequest},
        AppState,
    },
    storage::engine::CacheValue,
};
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use std::{net::SocketAddr, time::Duration};
use tokio::task::JoinHandle;

pub fn start(state: AppState, port: u16) -> JoinHandle<()> {
    tokio::spawn(async move {
        let router = Router::new()
            .route("/internal/get", post(handle_get))
            .route("/internal/set", post(handle_set))
            .route("/internal/delete", post(handle_delete))
            .route("/internal/gossip", post(handle_gossip))
            .route("/internal/health", get(handle_health))
            .with_state(state);

        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        tracing::info!("Internal node server listening on {}", addr);
        axum::serve(listener, router).await.unwrap();
    })
}

async fn handle_get(
    State(state): State<AppState>,
    Json(req): Json<GetRequest>,
) -> impl IntoResponse {
    match state.storage.get(&req.key) {
        Some(val) => Json(GetResponse {
            found: true,
            value: Some(val.data),
            expires_at_unix: val.expires_at_unix,
            created_at_unix: val.created_at_unix,
        }),
        None => Json(GetResponse {
            found: false,
            value: None,
            expires_at_unix: None,
            created_at_unix: 0,
        }),
    }
}

async fn handle_set(
    State(state): State<AppState>,
    Json(req): Json<SetRequest>,
) -> impl IntoResponse {
    let ttl = req.ttl_secs.map(Duration::from_secs);
    let value = CacheValue::new(req.value.clone(), ttl);

    let _ = state.wal.log_set(&req.key, &req.value, req.ttl_secs);
    state.storage.set(req.key, value);
    state.metrics.record_request("replicate", "ok");

    Json(OperationResponse {
        success: true,
        message: None,
    })
}

async fn handle_delete(
    State(state): State<AppState>,
    Json(req): Json<DeleteRequest>,
) -> impl IntoResponse {
    let _ = state.wal.log_delete(&req.key);
    let deleted = state.storage.delete(&req.key);
    state.metrics.record_request("replicate_delete", "ok");

    Json(OperationResponse {
        success: deleted,
        message: None,
    })
}

async fn handle_gossip(
    State(state): State<AppState>,
    Json(msg): Json<GossipMessage>,
) -> impl IntoResponse {
    state.cluster.merge_gossip(msg.nodes);
    let total = state.cluster.node_count();
    state.metrics.set_cluster_nodes(total as f64);
    StatusCode::OK
}

async fn handle_health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}
