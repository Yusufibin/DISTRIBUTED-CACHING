use crate::{
    config::Config,
    persistence::wal::WriteAheadLog,
    storage::engine::{CacheValue, StorageEngine},
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{fs, task::JoinHandle, time};

#[derive(Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub entries: Vec<(String, CacheValue)>,
    pub checkpoint: Option<([u8; 16], u64)>,
}

pub fn start_snapshot_loop(
    engine: Arc<StorageEngine>,
    wal: Arc<WriteAheadLog>,
    config: Config,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(config.snapshot_interval_secs));
        interval.tick().await;
        loop {
            interval.tick().await;
            let engine = engine.clone();
            let wal = wal.clone();
            match tokio::task::spawn_blocking(move || wal.snapshot(&engine)).await {
                Ok(Ok(())) => tracing::info!("Snapshot committed"),
                result => tracing::error!(?result, "Snapshot failed"),
            }
        }
    })
}

pub async fn load_latest_snapshot(data_dir: &str) -> anyhow::Result<Snapshot> {
    let dir = PathBuf::from(data_dir);
    let current = dir.join("snapshot-current.bin");
    if current.exists() {
        let data = fs::read(current).await?;
        anyhow::ensure!(data.starts_with(b"DCS2"), "invalid snapshot header");
        return Ok(bincode::deserialize(&data[4..])?);
    }
    let mut snapshots = Vec::new();
    let mut entries = fs::read_dir(&dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("bin")
            && path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with("snapshot-"))
        {
            snapshots.push(path);
        }
    }
    snapshots.sort();
    match snapshots.last() {
        Some(path) => Ok(Snapshot {
            entries: bincode::deserialize(&fs::read(path).await?)?,
            checkpoint: None,
        }),
        None => Ok(Snapshot::default()),
    }
}
