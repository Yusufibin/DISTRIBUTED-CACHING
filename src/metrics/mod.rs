use anyhow::Result;
use once_cell::sync::Lazy;
use prometheus::{
    register_counter_vec, register_gauge, register_histogram_vec, CounterVec, Gauge, HistogramVec,
    TextEncoder,
};

static CACHE_REQUESTS: Lazy<CounterVec> = Lazy::new(|| {
    register_counter_vec!(
        "cache_requests_total",
        "Total number of cache requests",
        &["operation", "result"]
    )
    .unwrap()
});

static CACHE_DURATION: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec!(
        "cache_request_duration_seconds",
        "Cache request duration",
        &["operation"],
        vec![0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5]
    )
    .unwrap()
});

static CACHE_ENTRIES: Lazy<Gauge> = Lazy::new(|| {
    register_gauge!("cache_entries_total", "Total entries in cache").unwrap()
});

static CACHE_EVICTIONS: Lazy<CounterVec> = Lazy::new(|| {
    register_counter_vec!("cache_evictions_total", "Total evictions", &["reason"]).unwrap()
});

static CLUSTER_NODES: Lazy<Gauge> = Lazy::new(|| {
    register_gauge!("cluster_nodes_total", "Number of known cluster nodes").unwrap()
});

static REPLICATION_OPS: Lazy<CounterVec> = Lazy::new(|| {
    register_counter_vec!(
        "replication_ops_total",
        "Replication operations",
        &["result"]
    )
    .unwrap()
});

static WAL_WRITES: Lazy<CounterVec> = Lazy::new(|| {
    register_counter_vec!("wal_writes_total", "WAL write operations", &["type"]).unwrap()
});

pub struct Metrics;

impl Metrics {
    pub fn new() -> Result<Self> {
        Lazy::force(&CACHE_REQUESTS);
        Lazy::force(&CACHE_DURATION);
        Lazy::force(&CACHE_ENTRIES);
        Lazy::force(&CACHE_EVICTIONS);
        Lazy::force(&CLUSTER_NODES);
        Lazy::force(&REPLICATION_OPS);
        Lazy::force(&WAL_WRITES);
        Ok(Metrics)
    }

    pub fn record_request(&self, op: &str, result: &str) {
        CACHE_REQUESTS.with_label_values(&[op, result]).inc();
    }

    pub fn observe_duration(&self, op: &str, seconds: f64) {
        CACHE_DURATION.with_label_values(&[op]).observe(seconds);
    }

    pub fn set_entries(&self, count: f64) {
        CACHE_ENTRIES.set(count);
    }

    pub fn record_eviction(&self, reason: &str) {
        CACHE_EVICTIONS.with_label_values(&[reason]).inc();
    }

    pub fn set_cluster_nodes(&self, count: f64) {
        CLUSTER_NODES.set(count);
    }

    pub fn record_replication(&self, result: &str) {
        REPLICATION_OPS.with_label_values(&[result]).inc();
    }

    #[allow(dead_code)]
    pub fn record_wal_write(&self, entry_type: &str) {
        WAL_WRITES.with_label_values(&[entry_type]).inc();
    }

    pub fn gather() -> Result<String> {
        let encoder = TextEncoder::new();
        let families = prometheus::gather();
        Ok(encoder.encode_to_string(&families)?)
    }
}
