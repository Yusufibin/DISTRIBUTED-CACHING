pub mod gossip;

use crate::config::Config;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    Healthy,
    Unhealthy,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: String,
    pub addr: String,
    pub node_port: u16,
    pub status: NodeStatus,
    pub last_seen: u64,
}

impl NodeInfo {
    #[allow(dead_code)]
    pub fn internal_addr(&self) -> String {
        let host = self.addr.split(':').next().unwrap_or(&self.addr);
        format!("{}:{}", host, self.node_port)
    }
}

pub struct ClusterState {
    nodes: DashMap<String, NodeInfo>,
    local_id: String,
    stale_threshold: Duration,
}

impl ClusterState {
    pub fn new(config: Config) -> Self {
        let state = ClusterState {
            nodes: DashMap::new(),
            local_id: config.node_id.clone(),
            stale_threshold: Duration::from_secs(config.heartbeat_interval_secs * 3),
        };

        let local = NodeInfo {
            id: config.node_id.clone(),
            addr: config.node_addr(),
            node_port: config.node_port,
            status: NodeStatus::Healthy,
            last_seen: current_unix_secs(),
        };
        state.nodes.insert(config.node_id.clone(), local);

        for peer_addr in &config.peers {
            let node_id = crate::router::peer_to_node_id(peer_addr);
            let info = NodeInfo {
                id: node_id.clone(),
                addr: peer_addr.clone(),
                node_port: config.node_port,
                status: NodeStatus::Unknown,
                last_seen: 0,
            };
            state.nodes.insert(node_id, info);
        }

        state
    }

    pub fn all_nodes(&self) -> Vec<NodeInfo> {
        self.nodes.iter().map(|e| e.value().clone()).collect()
    }

    #[allow(dead_code)]
    pub fn healthy_nodes(&self) -> Vec<NodeInfo> {
        self.nodes
            .iter()
            .filter(|e| e.value().status == NodeStatus::Healthy)
            .map(|e| e.value().clone())
            .collect()
    }

    pub fn node_address(&self, node_id: &str) -> Option<String> {
        self.nodes.get(node_id).map(|n| n.addr.clone())
    }

    pub fn mark_healthy(&self, node_id: &str) {
        if let Some(mut node) = self.nodes.get_mut(node_id) {
            node.status = NodeStatus::Healthy;
            node.last_seen = current_unix_secs();
        }
    }

    pub fn mark_unhealthy(&self, node_id: &str) {
        if let Some(mut node) = self.nodes.get_mut(node_id) {
            node.status = NodeStatus::Unhealthy;
        }
    }

    pub fn update_last_seen(&self, node_id: &str) {
        if let Some(mut node) = self.nodes.get_mut(node_id) {
            node.last_seen = current_unix_secs();
        }
    }

    pub fn merge_gossip(&self, nodes: Vec<NodeInfo>) {
        let now = current_unix_secs();
        for remote in nodes {
            if remote.id == self.local_id {
                continue;
            }
            self.nodes
                .entry(remote.id.clone())
                .and_modify(|local| {
                    if remote.last_seen > local.last_seen {
                        local.status = remote.status.clone();
                        local.last_seen = remote.last_seen;
                    }
                })
                .or_insert(remote);
        }

        let stale_secs = self.stale_threshold.as_secs();
        for mut node in self.nodes.iter_mut() {
            if node.id == self.local_id {
                continue;
            }
            if node.last_seen > 0 && now - node.last_seen > stale_secs {
                node.status = NodeStatus::Unhealthy;
            }
        }
    }

    #[allow(dead_code)]
    pub fn upsert_node(&self, info: NodeInfo) {
        self.nodes.insert(info.id.clone(), info);
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
