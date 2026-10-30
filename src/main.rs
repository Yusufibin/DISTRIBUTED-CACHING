use clap::Parser;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod cluster;
mod config;
mod error;
mod metrics;
mod network;
mod persistence;
mod router;
mod storage;

use config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,distributed_cache=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::parse();
    config.validate()?;

    tracing::info!(
        node_id = %config.node_id,
        http_port = config.http_port,
        node_port = config.node_port,
        peers = config.peers.len(),
        "Starting distributed cache node"
    );

    let metrics = Arc::new(metrics::Metrics::new()?);
    let engine = Arc::new(storage::engine::StorageEngine::new(
        config.clone(),
        metrics.clone(),
    ));

    let wal = Arc::new(persistence::wal::WriteAheadLog::new(&config.data_dir)?);

    let snapshot = persistence::snapshot::load_latest_snapshot(&config.data_dir).await?;
    engine.restore_entries(snapshot.entries);
    wal.replay(&engine, snapshot.checkpoint).await?;

    let cluster_state = Arc::new(cluster::ClusterState::new(config.clone()));
    let router = Arc::new(router::SmartRouter::new(
        config.clone(),
        cluster_state.clone(),
    ));

    metrics.set_cluster_nodes(cluster_state.node_count() as f64);

    let app_state = network::AppState {
        storage: engine.clone(),
        router: router.clone(),
        cluster: cluster_state.clone(),
        wal: wal.clone(),
        metrics: metrics.clone(),
        config: config.clone(),
    };

    let eviction_handle = storage::eviction::start_eviction_loop(engine.clone());

    let gossip_handle = cluster::gossip::start_gossip(cluster_state.clone(), config.clone());

    let heartbeat_handle = cluster::gossip::start_heartbeat(cluster_state.clone(), config.clone());

    let snapshot_handle =
        persistence::snapshot::start_snapshot_loop(engine.clone(), wal.clone(), config.clone());

    let http_handle = network::http_server::start(app_state.clone(), config.http_port);
    let node_handle = network::node_server::start(app_state.clone(), config.node_port);

    tokio::select! {
        _ = http_handle => {
            anyhow::bail!("HTTP server exited unexpectedly");
        }
        _ = node_handle => {
            anyhow::bail!("Node server exited unexpectedly");
        }
        _ = gossip_handle => {
            anyhow::bail!("Gossip task exited unexpectedly");
        }
        _ = heartbeat_handle => {
            anyhow::bail!("Heartbeat task exited unexpectedly");
        }
        _ = eviction_handle => {
            anyhow::bail!("Eviction task exited unexpectedly");
        }
        _ = snapshot_handle => {
            anyhow::bail!("Snapshot task exited unexpectedly");
        }
        result = tokio::signal::ctrl_c() => {
            match result {
                Ok(_) => tracing::info!("SIGINT received, shutting down gracefully"),
                Err(e) => tracing::error!("Signal handler error: {}", e),
            }
        }
    }

    tracing::info!("Node {} stopped", config.node_id);
    Ok(())
}

#[cfg(test)]
mod tests;
