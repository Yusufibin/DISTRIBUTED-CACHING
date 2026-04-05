pub mod circuit_breaker;
pub mod consistent_hash;

use crate::{
    cluster::ClusterState,
    config::Config,
    error::{CacheError, Result},
    router::{circuit_breaker::CircuitBreaker, consistent_hash::ConsistentHashRing},
};
use dashmap::DashMap;
use parking_lot::RwLock;
use std::{sync::Arc, time::Duration};

pub struct SmartRouter {
    ring: RwLock<ConsistentHashRing>,
    breakers: DashMap<String, Arc<CircuitBreaker>>,
    cluster: Arc<ClusterState>,
    config: Config,
}

impl SmartRouter {
    pub fn new(config: Config, cluster: Arc<ClusterState>) -> Self {
        let mut ring = ConsistentHashRing::new(config.virtual_nodes);
        ring.add_node(&config.node_id);
        for peer in &config.peers {
            let node_id = peer_to_node_id(peer);
            ring.add_node(&node_id);
        }

        SmartRouter {
            ring: RwLock::new(ring),
            breakers: DashMap::new(),
            cluster,
            config,
        }
    }

    pub fn primary_node(&self, key: &str) -> Result<String> {
        let ring = self.ring.read();
        ring.get_node(key)
            .map(|s| s.to_string())
            .ok_or_else(|| CacheError::NoNodeAvailable(key.to_string()))
    }

    pub fn replica_nodes(&self, key: &str) -> Vec<String> {
        let ring = self.ring.read();
        ring.get_nodes(key, self.config.replication_factor)
    }

    pub fn is_local(&self, key: &str) -> bool {
        self.primary_node(key)
            .map(|n| n == self.config.node_id)
            .unwrap_or(false)
    }

    pub fn node_address(&self, node_id: &str) -> Option<String> {
        if node_id == self.config.node_id {
            return Some(self.config.node_addr());
        }
        self.cluster.node_address(node_id)
    }

    pub fn circuit_breaker_for(&self, node_id: &str) -> Arc<CircuitBreaker> {
        self.breakers
            .entry(node_id.to_string())
            .or_insert_with(|| {
                Arc::new(CircuitBreaker::new(
                    self.config.circuit_breaker_threshold,
                    Duration::from_secs(self.config.circuit_breaker_timeout_secs),
                ))
            })
            .clone()
    }

    pub fn is_circuit_open(&self, node_id: &str) -> bool {
        let cb = self.circuit_breaker_for(node_id);
        cb.is_open()
    }

    pub fn record_success(&self, node_id: &str) {
        let cb = self.circuit_breaker_for(node_id);
        cb.record_success();
    }

    pub fn record_failure(&self, node_id: &str) {
        let cb = self.circuit_breaker_for(node_id);
        cb.record_failure();
    }

    #[allow(dead_code)]
    pub fn add_node(&self, node_id: &str) {
        self.ring.write().add_node(node_id);
        tracing::info!("Added node {} to hash ring", node_id);
    }

    #[allow(dead_code)]
    pub fn remove_node(&self, node_id: &str) {
        self.ring.write().remove_node(node_id);
        tracing::info!("Removed node {} from hash ring", node_id);
    }

    pub fn best_available_node(&self, key: &str) -> Result<String> {
        let ring = self.ring.read();
        let candidates = ring.get_nodes(key, 5);
        drop(ring);

        for node in candidates {
            if !self.is_circuit_open(&node) {
                return Ok(node);
            }
        }
        Err(CacheError::NoNodeAvailable(key.to_string()))
    }
}

pub fn peer_to_node_id(peer_addr: &str) -> String {
    format!("node-{}", peer_addr.replace([':', '.'], "-"))
}
