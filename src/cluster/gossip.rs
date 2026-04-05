use crate::{cluster::ClusterState, config::Config, network::node_client::NodeClient};
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use tokio::task::JoinHandle;
use tokio::time;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipMessage {
    pub from_id: String,
    pub nodes: Vec<super::NodeInfo>,
    pub timestamp: u64,
}

pub fn start_gossip(state: Arc<ClusterState>, config: Config) -> JoinHandle<()> {
    tokio::spawn(async move {
        let client = NodeClient::new(config.request_timeout_secs);
        let mut interval = time::interval(Duration::from_secs(config.gossip_interval_secs));
        loop {
            interval.tick().await;
            let peers: Vec<String> = state
                .all_nodes()
                .into_iter()
                .filter(|n| n.id != config.node_id)
                .map(|n| n.addr)
                .collect();

            if peers.is_empty() {
                continue;
            }

            let sample_size = (peers.len() / 2).max(1);
            let targets: Vec<String> = {
                let mut rng = rand::thread_rng();
                peers.choose_multiple(&mut rng, sample_size).cloned().collect()
            };

            let msg = GossipMessage {
                from_id: config.node_id.clone(),
                nodes: state.all_nodes(),
                timestamp: current_unix_secs(),
            };

            for target in targets {
                match client.post_gossip(&target, &msg).await {
                    Ok(_) => state.update_last_seen(&config.node_id),
                    Err(e) => tracing::warn!("Gossip to {} failed: {}", target, e),
                }
            }
        }
    })
}

pub fn start_heartbeat(state: Arc<ClusterState>, config: Config) -> JoinHandle<()> {
    tokio::spawn(async move {
        let client = NodeClient::new(config.request_timeout_secs);
        let mut interval = time::interval(Duration::from_secs(config.heartbeat_interval_secs));
        loop {
            interval.tick().await;
            let nodes = state.all_nodes();
            for node in nodes.iter().filter(|n| n.id != config.node_id) {
                if client.health_check(&node.addr).await {
                    state.mark_healthy(&node.id);
                } else {
                    state.mark_unhealthy(&node.id);
                    tracing::warn!("Node {} is unreachable", node.id);
                }
            }
        }
    })
}

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
