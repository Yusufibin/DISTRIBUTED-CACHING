use crate::{
    cluster::ClusterState,
    config::Config,
    metrics::Metrics,
    persistence::{snapshot::load_latest_snapshot, wal::WriteAheadLog},
    router::SmartRouter,
    storage::engine::{CacheValue, StorageEngine},
};
use clap::Parser;
use std::{io::Write, path::PathBuf, sync::Arc, time::Duration};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("cache-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn name(&self) -> &str {
        self.0.to_str().unwrap()
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn engine() -> Arc<StorageEngine> {
    Arc::new(StorageEngine::new(
        Config::parse_from(["test"]),
        Arc::new(Metrics::new().unwrap()),
    ))
}
fn value(data: &str) -> CacheValue {
    CacheValue::new(data.as_bytes().to_vec(), None)
}

#[test]
fn lru_preserves_recently_read_entry() {
    let engine = engine();
    engine.set("first".into(), value("a"));
    engine.set("second".into(), value("b"));
    engine.get("first").unwrap();
    assert_eq!(engine.evict_lru(1), 1);
    assert!(engine.get("first").is_some());
    assert!(engine.get("second").is_none());
    assert_eq!(engine.len(), 1);
}

#[test]
fn concurrent_overwrites_and_expiration_keep_exact_count() {
    let engine = engine();
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let engine = engine.clone();
            scope.spawn(move || {
                for i in 0..1000 {
                    engine.set(
                        "shared".into(),
                        if i % 2 == 0 {
                            CacheValue::new(vec![], Some(Duration::ZERO))
                        } else {
                            value("fresh")
                        },
                    );
                    engine.get("shared");
                    engine.evict_expired();
                    engine.delete("shared");
                }
            });
        }
    });
    engine.set("shared".into(), value("final"));
    assert_eq!(engine.len(), 1);
    engine.restore_entries(vec![("shared".into(), value("restored"))]);
    assert_eq!(engine.len(), 1);
    assert!(engine.delete("shared"));
    assert!(!engine.delete("shared"));
    assert_eq!(engine.len(), 0);
}

#[tokio::test]
async fn wal_preserves_timestamps_and_does_not_resurrect_expired_overwrite() {
    let dir = Directory::new();
    let engine = engine();
    let wal = WriteAheadLog::new(dir.name()).unwrap();
    wal.set(&engine, "key".into(), value("old")).unwrap();
    let expired = CacheValue {
        data: b"new".to_vec(),
        created_at_unix: 1,
        expires_at_unix: Some(2),
    };
    wal.set(&engine, "key".into(), expired).unwrap();
    let permanent = value("keep");
    wal.set(&engine, "keep".into(), permanent.clone()).unwrap();
    let recovered = super::tests::engine();
    wal.replay(&recovered, None).await.unwrap();
    assert!(recovered.get("key").is_none());
    assert_eq!(
        recovered.get("keep").unwrap().created_at_unix,
        permanent.created_at_unix
    );
}

#[tokio::test]
async fn snapshot_recovers_before_and_after_wal_replacement() {
    let dir = Directory::new();
    let original = engine();
    let wal = WriteAheadLog::new(dir.name()).unwrap();
    wal.set(&original, "evicted".into(), value("old")).unwrap();
    wal.set(&original, "keep".into(), value("snapshot"))
        .unwrap();
    let old_wal = std::fs::read(dir.0.join("cache.wal")).unwrap();
    original.delete("evicted");
    wal.snapshot(&original).unwrap();
    wal.set(&original, "keep".into(), value("updated")).unwrap();
    wal.set(&original, "deleted".into(), value("gone")).unwrap();
    wal.delete(&original, "deleted").unwrap();
    let snapshot = load_latest_snapshot(dir.name()).await.unwrap();
    let recovered = engine();
    recovered.restore_entries(snapshot.entries);
    wal.replay(&recovered, snapshot.checkpoint).await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered.get("keep").unwrap().data, b"updated");
    drop(wal);
    // Simulate power loss after snapshot rename but before WAL replacement.
    std::fs::write(dir.0.join("cache.wal"), old_wal).unwrap();
    let snapshot = load_latest_snapshot(dir.name()).await.unwrap();
    let recovered = engine();
    recovered.restore_entries(snapshot.entries);
    WriteAheadLog::new(dir.name())
        .unwrap()
        .replay(&recovered, snapshot.checkpoint)
        .await
        .unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered.get("keep").unwrap().data, b"snapshot");
}

#[tokio::test]
async fn incomplete_tail_is_repaired_before_new_writes() {
    for tail in [vec![0], vec![0; 6], vec![0, 0, 0, 0, 10, 0, 0, 0, 1, 2]] {
        let dir = Directory::new();
        let original = engine();
        let wal = WriteAheadLog::new(dir.name()).unwrap();
        wal.set(&original, "first".into(), value("one")).unwrap();
        drop(wal);
        std::fs::OpenOptions::new()
            .append(true)
            .open(dir.0.join("cache.wal"))
            .unwrap()
            .write_all(&tail)
            .unwrap();
        let wal = WriteAheadLog::new(dir.name()).unwrap();
        wal.replay(&original, None).await.unwrap();
        wal.set(&original, "second".into(), value("two")).unwrap();
        let recovered = engine();
        wal.replay(&recovered, None).await.unwrap();
        assert_eq!(recovered.len(), 2);
    }
}

#[tokio::test]
async fn corrupt_wal_and_snapshot_are_reported() {
    let dir = Directory::new();
    std::fs::write(dir.0.join("cache.wal"), b"bogus").unwrap();
    assert!(WriteAheadLog::new(dir.name()).is_err());
    std::fs::remove_file(dir.0.join("cache.wal")).unwrap();
    let wal = WriteAheadLog::new(dir.name()).unwrap();
    wal.set(&engine(), "key".into(), value("data")).unwrap();
    drop(wal);
    let path = dir.0.join("cache.wal");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[21] ^= 1;
    std::fs::write(path, bytes).unwrap();
    assert!(WriteAheadLog::new(dir.name())
        .unwrap()
        .replay(&engine(), None)
        .await
        .is_err());
    std::fs::write(dir.0.join("snapshot-current.bin"), b"broken").unwrap();
    assert!(load_latest_snapshot(dir.name()).await.is_err());
}

#[test]
fn all_nodes_agree_on_owner_despite_custom_node_names() {
    let configs: Vec<_> = (0..3)
        .map(|i| {
            let mut config = Config::parse_from(["test"]);
            config.node_id = format!("custom-{i}");
            config.http_port = 18080 + i;
            config.peers = (0..3)
                .filter(|j| *j != i)
                .map(|j| format!("127.0.0.1:{}", 18080 + j))
                .collect();
            config
        })
        .collect();
    let routers: Vec<_> = configs
        .into_iter()
        .map(|config| SmartRouter::new(config.clone(), Arc::new(ClusterState::new(config))))
        .collect();
    for i in 0..1000 {
        let key = format!("key-{i}");
        let owner = routers[0].primary_node(&key).unwrap();
        assert!(routers
            .iter()
            .all(|router| router.primary_node(&key).unwrap() == owner));
        assert_eq!(
            routers
                .iter()
                .filter(|router| router.is_local(&key))
                .count(),
            1
        );
    }
}

#[test]
fn zero_replicas_returns_no_nodes() {
    let mut ring = crate::router::consistent_hash::ConsistentHashRing::new(16);
    ring.add_node("a");
    assert!(ring.get_nodes("key", 0).is_empty());
}

#[test]
fn invalid_configuration_is_rejected() {
    for args in [
        vec!["test", "--snapshot-interval-secs", "0"],
        vec!["test", "--virtual-nodes", "0"],
        vec!["test", "--host", "0.0.0.0"],
        vec!["test", "--peers", "bad-peer"],
    ] {
        assert!(Config::parse_from(args).validate().is_err());
    }
}

#[tokio::test]
async fn concurrent_snapshots_preserve_all_acknowledged_writes() {
    let dir = Directory::new();
    let original = engine();
    let wal = Arc::new(WriteAheadLog::new(dir.name()).unwrap());
    std::thread::scope(|scope| {
        for writer in 0..4 {
            let wal = wal.clone();
            let original = original.clone();
            scope.spawn(move || {
                for i in 0..50 {
                    wal.set(&original, format!("{writer}-{i}"), value("data"))
                        .unwrap();
                }
            });
        }
        let wal = wal.clone();
        let original = original.clone();
        scope.spawn(move || {
            for _ in 0..10 {
                wal.snapshot(&original).unwrap();
            }
        });
    });
    let snapshot = load_latest_snapshot(dir.name()).await.unwrap();
    let recovered = engine();
    recovered.restore_entries(snapshot.entries);
    wal.replay(&recovered, snapshot.checkpoint).await.unwrap();
    assert_eq!(recovered.len(), 200);
    for writer in 0..4 {
        for i in 0..50 {
            assert_eq!(
                recovered.get(&format!("{writer}-{i}")).unwrap().data,
                b"data"
            );
        }
    }
}

#[tokio::test]
async fn legacy_wal_is_still_readable() {
    let dir = Directory::new();
    let data = bincode::serialize(&crate::persistence::wal::WalEntry::Set {
        key: "legacy".into(),
        value: b"old".to_vec(),
        ttl_secs: None,
    })
    .unwrap();
    let mut file = std::fs::File::create(dir.0.join("cache.wal")).unwrap();
    file.write_all(b"DCWL\x01").unwrap();
    file.write_all(&crc32fast::hash(&data).to_le_bytes())
        .unwrap();
    file.write_all(&(data.len() as u32).to_le_bytes()).unwrap();
    file.write_all(&data).unwrap();
    let recovered = engine();
    WriteAheadLog::new(dir.name())
        .unwrap()
        .replay(&recovered, None)
        .await
        .unwrap();
    assert_eq!(recovered.get("legacy").unwrap().data, b"old");
}

#[tokio::test]
async fn unreachable_or_stalled_peer_respects_timeout() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server = tokio::spawn(async move {
        let (_connection, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let client = crate::network::node_client::NodeClient::new(1);
    let started = std::time::Instant::now();
    assert!(client.get(&addr, "key").await.is_err());
    assert!(started.elapsed() < Duration::from_secs(3));
    server.abort();
    assert!(!client.health_check("invalid address").await);
}

#[test]
fn circuit_recovery_requires_consecutive_successes() {
    let breaker = crate::router::circuit_breaker::CircuitBreaker::new(1, Duration::ZERO);
    breaker.record_failure();
    assert!(!breaker.is_open());
    breaker.record_success();
    breaker.record_success();
    breaker.record_failure();
    assert!(!breaker.is_open());
    breaker.record_success();
    assert_eq!(breaker.state_str(), "half-open");
    breaker.record_success();
    breaker.record_success();
    assert_eq!(breaker.state_str(), "closed");
}

#[test]
fn distinct_peer_addresses_have_distinct_ring_identities() {
    assert_ne!(
        crate::router::peer_to_node_id("node.example:8080"),
        crate::router::peer_to_node_id("node-example:8080")
    );
}
