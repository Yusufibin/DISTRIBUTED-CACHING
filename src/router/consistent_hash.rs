use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub struct ConsistentHashRing {
    ring: BTreeMap<u64, String>,
    virtual_nodes: usize,
}

impl ConsistentHashRing {
    pub fn new(virtual_nodes: usize) -> Self {
        ConsistentHashRing {
            ring: BTreeMap::new(),
            virtual_nodes,
        }
    }

    fn hash(key: &str) -> u64 {
        let mut hasher = Sha256::new();
        hasher.update(key.as_bytes());
        let result = hasher.finalize();
        u64::from_be_bytes(result[..8].try_into().unwrap())
    }

    pub fn add_node(&mut self, node_id: &str) {
        for i in 0..self.virtual_nodes {
            let vnode_key = format!("{}-vnode-{}", node_id, i);
            let hash = Self::hash(&vnode_key);
            self.ring.insert(hash, node_id.to_string());
        }
    }

    #[allow(dead_code)]
    pub fn remove_node(&mut self, node_id: &str) {
        for i in 0..self.virtual_nodes {
            let vnode_key = format!("{}-vnode-{}", node_id, i);
            let hash = Self::hash(&vnode_key);
            self.ring.remove(&hash);
        }
    }

    pub fn get_node(&self, key: &str) -> Option<&str> {
        if self.ring.is_empty() {
            return None;
        }
        let hash = Self::hash(key);
        let node = self
            .ring
            .range(hash..)
            .next()
            .or_else(|| self.ring.iter().next())
            .map(|(_, v)| v.as_str());
        node
    }

    pub fn get_nodes(&self, key: &str, count: usize) -> Vec<String> {
        if self.ring.is_empty() {
            return vec![];
        }
        let hash = Self::hash(key);
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();

        let forward = self.ring.range(hash..).chain(self.ring.iter());
        for (_, node_id) in forward {
            if seen.insert(node_id.clone()) {
                result.push(node_id.clone());
            }
            if result.len() >= count {
                break;
            }
        }
        result
    }

    #[allow(dead_code)]
    pub fn node_count(&self) -> usize {
        let unique: std::collections::HashSet<_> = self.ring.values().collect();
        unique.len()
    }
}
