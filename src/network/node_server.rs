use crate::{
    cluster::gossip::GossipMessage,
    network::{
        node_client::{DeleteRequest, GetRequest, GetResponse, OperationResponse, SetRequest},
        AppState,
    },
};
use axum::{
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use std::net::SocketAddr;
use tokio::task::JoinHandle;

pub fn start(state: AppState, port: u16) -> JoinHandle<()> {
    tokio::spawn(async move {
        let router = routes().with_state(state);

        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        tracing::info!("Internal node server listening on {}", addr);
        axum::serve(listener, router).await.unwrap();
    })
}

// Both listeners accept the internal protocol: configured peers use public addresses.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/internal/get", post(handle_get))
        .route("/internal/set", post(handle_set))
        .route("/internal/delete", post(handle_delete))
        .route("/internal/gossip", post(handle_gossip))
        .route("/internal/health", get(handle_health))
        .layer(DefaultBodyLimit::max(80 * 1024 * 1024))
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

async fn handle_set(State(state): State<AppState>, Json(req): Json<SetRequest>) -> Response {
    let value = req.value.clone();

    if let Err(error) = state.set(req.key.clone(), value).await {
        tracing::error!(%error, "Persistent write failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if state.router.is_local(&req.key) {
        super::http_server::replicate_to_peers(&state, &req.key, req.value).await;
    }
    state.metrics.record_request("replicate", "ok");

    Json(OperationResponse {
        success: true,
        message: None,
    })
    .into_response()
}

async fn handle_delete(State(state): State<AppState>, Json(req): Json<DeleteRequest>) -> Response {
    let deleted = match state.delete(req.key.clone()).await {
        Ok(deleted) => deleted,
        Err(error) => {
            tracing::error!(%error, "Persistent delete failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    if state.router.is_local(&req.key) {
        super::http_server::replicate_delete(&state, &req.key).await;
    }
    state.metrics.record_request("replicate_delete", "ok");

    Json(OperationResponse {
        success: deleted,
        message: None,
    })
    .into_response()
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
