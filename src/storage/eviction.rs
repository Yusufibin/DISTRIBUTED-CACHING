use crate::storage::engine::StorageEngine;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time;

pub fn start_eviction_loop(engine: Arc<StorageEngine>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let expired = engine.evict_expired();
            if expired > 0 {
                tracing::debug!("Evicted {} expired entries", expired);
            }

            let current = engine.len();
            let max = engine.max_entries();
            if current > max {
                let excess = current - max;
                let evicted = engine.evict_lru(excess + (max / 10));
                tracing::info!("LRU eviction: removed {} entries (capacity enforcement)", evicted);
            }
        }
    })
}
