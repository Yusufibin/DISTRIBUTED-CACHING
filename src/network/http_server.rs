use crate::{
    network::{node_client::NodeClient, AppState},
    storage::engine::CacheValue,
};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, time::Duration};
use tokio::task::JoinHandle;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

#[derive(Debug, Deserialize)]
pub struct SetQuery {
    pub ttl: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct GetResult {
    pub key: String,
    pub value: String,
    pub expires_at_unix: Option<u64>,
    pub created_at_unix: u64,
    pub served_by: String,
}

#[derive(Debug, Serialize)]
pub struct DeleteResult {
    pub key: String,
    pub deleted: bool,
}

#[derive(Debug, Serialize)]
pub struct ClusterInfo {
    pub node_id: String,
    pub nodes: Vec<crate::cluster::NodeInfo>,
    pub total_entries: u64,
}

pub fn start(state: AppState, port: u16) -> JoinHandle<()> {
    tokio::spawn(async move {
        let router = Router::new()
            .route("/v1/cache/:key", get(get_handler))
            .route("/v1/cache/:key", put(set_handler))
            .route("/v1/cache/:key", delete(delete_handler))
            .route("/v1/cluster/nodes", get(cluster_nodes_handler))
            .route("/metrics", get(metrics_handler))
            .route("/health", get(health_handler))
            .layer(TraceLayer::new_for_http())
            .layer(CorsLayer::permissive())
            .with_state(state);

        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        tracing::info!("HTTP API listening on {}", addr);
        axum::serve(listener, router).await.unwrap();
    })
}

async fn get_handler(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Response {
    let start = std::time::Instant::now();

    let result = if state.router.is_local(&key) {
        match state.storage.get(&key) {
            Some(val) => Ok(val),
            None => Err(StatusCode::NOT_FOUND),
        }
    } else {
        forward_get(&state, &key).await
    };

    let elapsed = start.elapsed().as_secs_f64();

    match result {
        Ok(val) => {
            state.metrics.record_request("get", "hit");
            state.metrics.observe_duration("get", elapsed);
            let body = String::from_utf8_lossy(&val.data).to_string();
            Json(GetResult {
                key,
                value: body,
                expires_at_unix: val.expires_at_unix,
                created_at_unix: val.created_at_unix,
                served_by: state.config.node_id.clone(),
            })
            .into_response()
        }
        Err(status) => {
            state.metrics.record_request("get", "miss");
            state.metrics.observe_duration("get", elapsed);
            status.into_response()
        }
    }
}

async fn set_handler(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Query(query): Query<SetQuery>,
    _headers: HeaderMap,
    body: Body,
) -> Response {
    let start = std::time::Instant::now();

    let bytes = match axum::body::to_bytes(body, 16 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    let ttl = query.ttl.or_else(|| Some(state.config.default_ttl_secs));

    if state.router.is_local(&key) {
        let value = CacheValue::new(bytes.to_vec(), ttl.map(Duration::from_secs));
        let _ = state.wal.log_set(&key, &bytes, ttl);
        state.storage.set(key.clone(), value.clone());
        state.metrics.record_request("set", "ok");
        state.metrics.observe_duration("set", start.elapsed().as_secs_f64());

        replicate_to_peers(&state, &key, bytes.to_vec(), ttl).await;
        StatusCode::OK.into_response()
    } else {
        forward_set(&state, &key, bytes.to_vec(), ttl).await
    }
}

async fn delete_handler(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Response {
    let start = std::time::Instant::now();

    if state.router.is_local(&key) {
        let _ = state.wal.log_delete(&key);
        let deleted = state.storage.delete(&key);
        state.metrics.record_request("delete", if deleted { "ok" } else { "miss" });
        state.metrics.observe_duration("delete", start.elapsed().as_secs_f64());
        Json(DeleteResult { key, deleted }).into_response()
    } else {
        forward_delete(&state, &key).await
    }
}

async fn cluster_nodes_handler(State(state): State<AppState>) -> impl IntoResponse {
    Json(ClusterInfo {
        node_id: state.config.node_id.clone(),
        nodes: state.cluster.all_nodes(),
        total_entries: state.storage.len(),
    })
}

async fn metrics_handler(State(_state): State<AppState>) -> Response {
    match crate::metrics::Metrics::gather() {
        Ok(text) => Response::builder()
            .header("content-type", "text/plain; charset=utf-8")
            .body(Body::from(text))
            .unwrap(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn health_handler(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "node_id": state.config.node_id,
        "entries": state.storage.len(),
        "cluster_nodes": state.cluster.node_count(),
    }))
}

async fn forward_get(state: &AppState, key: &str) -> Result<CacheValue, StatusCode> {
    let node = state
        .router
        .best_available_node(key)
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    if state.router.is_circuit_open(&node) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    let addr = state
        .router
        .node_address(&node)
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;

    let client = NodeClient::new(state.config.request_timeout_secs);
    match client.get(&addr, key).await {
        Ok(Some(val)) => {
            state.router.record_success(&node);
            Ok(val)
        }
        Ok(None) => {
            state.router.record_success(&node);
            Err(StatusCode::NOT_FOUND)
        }
        Err(_) => {
            state.router.record_failure(&node);
            Err(StatusCode::BAD_GATEWAY)
        }
    }
}

async fn forward_set(
    state: &AppState,
    key: &str,
    value: Vec<u8>,
    ttl: Option<u64>,
) -> Response {
    let node = match state.router.best_available_node(key) {
        Ok(n) => n,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };

    let addr = match state.router.node_address(&node) {
        Some(a) => a,
        None => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };

    let client = NodeClient::new(state.config.request_timeout_secs);
    match client.set(&addr, key, value, ttl).await {
        Ok(_) => {
            state.router.record_success(&node);
            StatusCode::OK.into_response()
        }
        Err(_) => {
            state.router.record_failure(&node);
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

async fn forward_delete(state: &AppState, key: &str) -> Response {
    let node = match state.router.best_available_node(key) {
        Ok(n) => n,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };

    let addr = match state.router.node_address(&node) {
        Some(a) => a,
        None => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };

    let client = NodeClient::new(state.config.request_timeout_secs);
    match client.delete(&addr, key).await {
        Ok(deleted) => {
            state.router.record_success(&node);
            Json(DeleteResult {
                key: key.to_string(),
                deleted,
            })
            .into_response()
        }
        Err(_) => {
            state.router.record_failure(&node);
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

async fn replicate_to_peers(state: &AppState, key: &str, value: Vec<u8>, ttl: Option<u64>) {
    let replicas = state.router.replica_nodes(key);
    let client = NodeClient::new(state.config.request_timeout_secs);

    for replica_id in replicas.iter().filter(|id| *id != &state.config.node_id) {
        if let Some(addr) = state.router.node_address(replica_id) {
            let key = key.to_string();
            let value = value.clone();
            let client = client.clone();
            let router = state.router.clone();
            let metrics = state.metrics.clone();
            let replica_id = replica_id.clone();

            tokio::spawn(async move {
                match client.set(&addr, &key, value, ttl).await {
                    Ok(_) => {
                        router.record_success(&replica_id);
                        metrics.record_replication("ok");
                    }
                    Err(e) => {
                        router.record_failure(&replica_id);
                        metrics.record_replication("error");
                        tracing::warn!("Replication to {} failed: {}", replica_id, e);
                    }
                }
            });
        }
    }
}


