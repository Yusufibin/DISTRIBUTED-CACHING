use crate::{
    config::Config,
    persistence::wal::WriteAheadLog,
    storage::engine::StorageEngine,
};
use bincode;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{fs, task::JoinHandle, time};

pub fn start_snapshot_loop(
    engine: Arc<StorageEngine>,
    wal: Arc<WriteAheadLog>,
    config: Config,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(config.snapshot_interval_secs));
        let data_dir = PathBuf::from(&config.data_dir);

        loop {
            interval.tick().await;

            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            let entries = engine.snapshot_entries();
            let path = data_dir.join(format!("snapshot-{}.bin", ts));

            match save_snapshot(&path, &entries).await {
                Ok(_) => {
                    tracing::info!("Snapshot saved: {} entries -> {:?}", entries.len(), path);
                    let _ = cleanup_old_snapshots(&data_dir, 3).await;
                    let _ = wal.log_checkpoint(ts);
                    let _ = wal.rotate();
                }
                Err(e) => {
                    tracing::error!("Snapshot failed: {}", e);
                }
            }
        }
    })
}

async fn save_snapshot(
    path: &PathBuf,
    entries: &Vec<(String, crate::storage::engine::CacheValue)>,
) -> anyhow::Result<()> {
    let data = bincode::serialize(entries)?;
    fs::write(path, &data).await?;
    Ok(())
}

pub async fn load_latest_snapshot(
    data_dir: &str,
) -> anyhow::Result<Vec<(String, crate::storage::engine::CacheValue)>> {
    let dir = PathBuf::from(data_dir);
    let mut snapshots: Vec<PathBuf> = Vec::new();

    let mut entries = fs::read_dir(&dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("bin")
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("snapshot-"))
                .unwrap_or(false)
        {
            snapshots.push(path);
        }
    }

    snapshots.sort();

    if let Some(latest) = snapshots.last() {
        let data = fs::read(latest).await?;
        let entries = bincode::deserialize(&data)?;
        tracing::info!("Loaded snapshot: {:?}", latest);
        return Ok(entries);
    }

    Ok(vec![])
}

async fn cleanup_old_snapshots(data_dir: &PathBuf, keep: usize) -> anyhow::Result<()> {
    let mut snapshots: Vec<PathBuf> = Vec::new();

    let mut entries = fs::read_dir(data_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("bin")
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("snapshot-"))
                .unwrap_or(false)
        {
            snapshots.push(path);
        }
    }

    snapshots.sort();

    if snapshots.len() > keep {
        for old in snapshots.iter().take(snapshots.len() - keep) {
            fs::remove_file(old).await?;
            tracing::debug!("Removed old snapshot: {:?}", old);
        }
    }

    Ok(())
}
