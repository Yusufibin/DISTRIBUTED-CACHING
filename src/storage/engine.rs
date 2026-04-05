use crate::{config::Config, metrics::Metrics};
use bytes::Bytes;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheValue {
    pub data: Vec<u8>,
    pub expires_at_unix: Option<u64>,
    pub created_at_unix: u64,
}

impl CacheValue {
    pub fn new(data: Vec<u8>, ttl: Option<Duration>) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        CacheValue {
            data,
            expires_at_unix: ttl.map(|t| now + t.as_secs()),
            created_at_unix: now,
        }
    }

    pub fn is_expired(&self) -> bool {
        if let Some(exp) = self.expires_at_unix {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            now >= exp
        } else {
            false
        }
    }

#[allow(dead_code)]
    pub fn to_bytes(&self) -> Bytes {
        Bytes::from(self.data.clone())
    }
}

struct Entry {
    value: CacheValue,
    access_count: AtomicU64,
    last_access: AtomicU64,
}

impl Entry {
    fn new(value: CacheValue) -> Self {
        let now = Instant::now().elapsed().as_millis() as u64;
        Entry {
            value,
            access_count: AtomicU64::new(0),
            last_access: AtomicU64::new(now),
        }
    }

    fn touch(&self) {
        self.access_count.fetch_add(1, Ordering::Relaxed);
        self.last_access.store(
            Instant::now().elapsed().as_millis() as u64,
            Ordering::Relaxed,
        );
    }
}

pub struct StorageEngine {
    store: Arc<DashMap<String, Arc<Entry>>>,
    config: Config,
    metrics: Arc<Metrics>,
    entry_count: AtomicU64,
}

impl StorageEngine {
    pub fn new(config: Config, metrics: Arc<Metrics>) -> Self {
        StorageEngine {
            store: Arc::new(DashMap::with_capacity(1024)),
            config,
            metrics,
            entry_count: AtomicU64::new(0),
        }
    }

    pub fn get(&self, key: &str) -> Option<CacheValue> {
        let entry = self.store.get(key)?;
        if entry.value.is_expired() {
            drop(entry);
            self.store.remove(key);
            self.entry_count.fetch_sub(1, Ordering::Relaxed);
            self.metrics.record_eviction("expired");
            return None;
        }
        entry.touch();
        Some(entry.value.clone())
    }

    pub fn set(&self, key: String, value: CacheValue) {
        let is_new = !self.store.contains_key(&key);
        self.store.insert(key, Arc::new(Entry::new(value)));
        if is_new {
            let count = self.entry_count.fetch_add(1, Ordering::Relaxed) + 1;
            self.metrics.set_entries(count as f64);
        }
    }

    pub fn delete(&self, key: &str) -> bool {
        if self.store.remove(key).is_some() {
            let count = self.entry_count.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
            self.metrics.set_entries(count as f64);
            true
        } else {
            false
        }
    }

    #[allow(dead_code)]
    pub fn contains(&self, key: &str) -> bool {
        self.store.contains_key(key)
    }

    pub fn len(&self) -> u64 {
        self.entry_count.load(Ordering::Relaxed)
    }

    pub fn evict_expired(&self) -> u64 {
        let mut evicted = 0u64;
        let expired_keys: Vec<String> = self
            .store
            .iter()
            .filter(|e| e.value().value.is_expired())
            .map(|e| e.key().clone())
            .collect();

        for key in expired_keys {
            if self.store.remove(&key).is_some() {
                evicted += 1;
                self.metrics.record_eviction("expired");
            }
        }

        if evicted > 0 {
            let count = self.entry_count.fetch_sub(evicted, Ordering::Relaxed).saturating_sub(evicted);
            self.metrics.set_entries(count as f64);
        }

        evicted
    }

    pub fn evict_lru(&self, target: u64) -> u64 {
        let mut entries: Vec<(String, u64)> = self
            .store
            .iter()
            .map(|e| (e.key().clone(), e.value().last_access.load(Ordering::Relaxed)))
            .collect();

        entries.sort_by_key(|(_, ts)| *ts);

        let to_evict = entries.iter().take(target as usize);
        let mut evicted = 0u64;
        for (key, _) in to_evict {
            if self.store.remove(key).is_some() {
                evicted += 1;
                self.metrics.record_eviction("lru");
            }
        }

        if evicted > 0 {
            let count = self.entry_count.fetch_sub(evicted, Ordering::Relaxed).saturating_sub(evicted);
            self.metrics.set_entries(count as f64);
        }

        evicted
    }

    #[allow(dead_code)]
    pub fn evict_lfu(&self, target: u64) -> u64 {
        let mut entries: Vec<(String, u64)> = self
            .store
            .iter()
            .map(|e| (e.key().clone(), e.value().access_count.load(Ordering::Relaxed)))
            .collect();

        entries.sort_by_key(|(_, count)| *count);

        let to_evict = entries.iter().take(target as usize);
        let mut evicted = 0u64;
        for (key, _) in to_evict {
            if self.store.remove(key).is_some() {
                evicted += 1;
                self.metrics.record_eviction("lfu");
            }
        }

        if evicted > 0 {
            let count = self.entry_count.fetch_sub(evicted, Ordering::Relaxed).saturating_sub(evicted);
            self.metrics.set_entries(count as f64);
        }

        evicted
    }

    pub fn snapshot_entries(&self) -> Vec<(String, CacheValue)> {
        self.store
            .iter()
            .filter(|e| !e.value().value.is_expired())
            .map(|e| (e.key().clone(), e.value().value.clone()))
            .collect()
    }

    pub fn restore_entries(&self, entries: Vec<(String, CacheValue)>) {
        for (key, value) in entries {
            if !value.is_expired() {
                self.store.insert(key, Arc::new(Entry::new(value)));
                self.entry_count.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.metrics.set_entries(self.entry_count.load(Ordering::Relaxed) as f64);
    }

    pub fn max_entries(&self) -> u64 {
        self.config.max_entries
    }
}
