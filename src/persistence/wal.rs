use crate::{
    error::Result,
    storage::engine::{CacheValue, StorageEngine},
};
use crc32fast::Hasher as Crc32Hasher;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

const WAL_MAGIC: &[u8; 4] = b"DCWL";
const WAL_VERSION: u8 = 2;
const MAX_RECORD_SIZE: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WalEntry {
    Set {
        key: String,
        value: Vec<u8>,
        ttl_secs: Option<u64>,
    },
    Delete {
        key: String,
    },
    Checkpoint {
        timestamp: u64,
    },
    SetValue {
        key: String,
        value: CacheValue,
    },
}

struct WalFile {
    file: File,
    generation: [u8; 16],
    header_len: u64,
    failed: bool,
}

impl WalFile {
    fn open_or_create(path: &Path) -> std::io::Result<Self> {
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)?;
        if file.metadata()?.len() == 0 {
            file.write_all(WAL_MAGIC)?;
            file.write_all(&[WAL_VERSION])?;
            file.write_all(uuid::Uuid::new_v4().as_bytes())?;
            file.sync_all()?;
        }
        file.seek(SeekFrom::Start(0))?;
        let mut header = [0; 5];
        file.read_exact(&mut header)?;
        if &header[..4] != WAL_MAGIC || ![1, 2].contains(&header[4]) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid WAL header",
            ));
        }
        let mut generation = [0; 16];
        let header_len = if header[4] == 2 {
            file.read_exact(&mut generation)?;
            21
        } else {
            5
        };
        Ok(WalFile {
            file,
            generation,
            header_len,
            failed: false,
        })
    }

    fn write_entry(&mut self, entry: &WalEntry) -> std::io::Result<()> {
        if self.failed {
            return Err(std::io::Error::other("WAL write failed; restart required"));
        }
        let data = bincode::serialize(entry)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

        let mut crc = Crc32Hasher::new();
        crc.update(&data);
        let checksum = crc.finalize();

        if data.len() > MAX_RECORD_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "WAL record too large",
            ));
        }
        let len = data.len() as u32;
        let result = (|| {
            self.file.write_all(&checksum.to_le_bytes())?;
            self.file.write_all(&len.to_le_bytes())?;
            self.file.write_all(&data)?;
            self.file.sync_data()
        })();
        self.failed = result.is_err();
        result
    }

    fn read_all_entries(&mut self) -> std::io::Result<Vec<WalEntry>> {
        self.file.seek(SeekFrom::Start(self.header_len))?;
        let mut entries = Vec::new();
        let file_len = self.file.metadata()?.len();
        loop {
            let start = self.file.stream_position()?;
            if start == file_len {
                break;
            }
            let mut header = [0; 8];
            match self.file.read_exact(&mut header) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    self.file.set_len(start)?;
                    self.file.sync_data()?;
                    break;
                }
                Err(error) => return Err(error),
            }
            let len = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
            if len > MAX_RECORD_SIZE {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid WAL record size",
                ));
            }
            if file_len - start - 8 < len as u64 {
                self.file.set_len(start)?;
                self.file.sync_data()?;
                break;
            }
            let mut data = vec![0; len];
            self.file.read_exact(&mut data)?;
            if crc32fast::hash(&data) != u32::from_le_bytes(header[..4].try_into().unwrap()) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "WAL checksum mismatch",
                ));
            }
            let entry = bincode::deserialize(&data)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            entries.push(entry);
        }
        Ok(entries)
    }
}

pub struct WriteAheadLog {
    inner: Mutex<WalFile>,
    path: PathBuf,
}

impl WriteAheadLog {
    pub fn new(data_dir: &str) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let path = PathBuf::from(data_dir).join("cache.wal");
        let wal_file = WalFile::open_or_create(&path)?;
        File::open(data_dir)?.sync_all()?;
        Ok(WriteAheadLog {
            inner: Mutex::new(wal_file),
            path,
        })
    }

    // The same mutex orders durable writes, in-memory mutations and snapshots.
    pub fn set(&self, engine: &StorageEngine, key: String, value: CacheValue) -> Result<()> {
        let mut wal = self.inner.lock();
        wal.write_entry(&WalEntry::SetValue {
            key: key.clone(),
            value: value.clone(),
        })?;
        engine.set(key, value);
        Ok(())
    }

    pub fn delete(&self, engine: &StorageEngine, key: &str) -> Result<bool> {
        let mut wal = self.inner.lock();
        wal.write_entry(&WalEntry::Delete {
            key: key.to_string(),
        })?;
        Ok(engine.delete(key))
    }

    pub async fn replay(
        &self,
        engine: &Arc<StorageEngine>,
        checkpoint: Option<([u8; 16], u64)>,
    ) -> Result<()> {
        let mut wal = self.inner.lock();
        // Parse the entire file to validate framing and repair an incomplete final record.
        let entries = wal.read_all_entries()?;
        let mut position = wal.header_len;
        for entry in entries {
            position += 8 + bincode::serialized_size(&entry)?;
            if checkpoint.is_some_and(|(generation, offset)| {
                generation == wal.generation && position <= offset
            }) {
                continue;
            }
            match entry {
                WalEntry::SetValue { key, value } => {
                    if value.is_expired() {
                        engine.delete(&key);
                    } else {
                        engine.set(key, value);
                    }
                }
                WalEntry::Set {
                    key,
                    value,
                    ttl_secs,
                } => {
                    // Legacy records did not store their original creation timestamp.
                    engine.set(
                        key,
                        CacheValue::new(value, ttl_secs.map(std::time::Duration::from_secs)),
                    );
                }
                WalEntry::Delete { key } => {
                    engine.delete(&key);
                }
                WalEntry::Checkpoint { .. } => {}
            }
        }
        Ok(())
    }

    pub fn snapshot(&self, engine: &StorageEngine) -> Result<()> {
        let mut wal = self.inner.lock();
        if wal.failed {
            return Err(std::io::Error::other("WAL write failed; restart required").into());
        }
        let dir = self.path.parent().unwrap();
        let snapshot = super::snapshot::Snapshot {
            entries: engine.snapshot_entries(),
            checkpoint: Some((wal.generation, wal.file.metadata()?.len())),
        };
        let temporary = dir.join("snapshot.tmp");
        let mut file = File::create(&temporary)?;
        file.write_all(b"DCS2")?;
        file.write_all(&bincode::serialize(&snapshot)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, dir.join("snapshot-current.bin"))?;
        File::open(dir)?.sync_all()?;

        // Replace the WAL atomically. A crash before replacement replays only the
        // suffix after the snapshot checkpoint; after replacement, all new records.
        let next = dir.join("cache.wal.tmp");
        if next.exists() {
            std::fs::remove_file(&next)?;
        }
        let new_wal = WalFile::open_or_create(&next)?;
        std::fs::rename(&next, &self.path)?;
        *wal = new_wal;
        if let Err(error) = File::open(dir).and_then(|directory| directory.sync_all()) {
            // The rename may not survive a crash. Do not acknowledge any writes
            // to this new generation until a restart establishes the on-disk state.
            wal.failed = true;
            return Err(error.into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod failure_tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn io_failure_does_not_mutate_memory_and_stops_further_writes() {
        let dir = std::env::temp_dir().join(format!("wal-failure-{}", uuid::Uuid::new_v4()));
        let wal = WriteAheadLog::new(dir.to_str().unwrap()).unwrap();
        let engine = StorageEngine::new(
            crate::config::Config::parse_from(["test"]),
            Arc::new(crate::metrics::Metrics::new().unwrap()),
        );
        engine.set("key".into(), CacheValue::new(b"old".to_vec(), None));
        wal.inner.lock().file = OpenOptions::new().write(true).open("/dev/full").unwrap();
        assert!(wal
            .set(
                &engine,
                "key".into(),
                CacheValue::new(b"new".to_vec(), None)
            )
            .is_err());
        assert!(wal.delete(&engine, "key").is_err());
        assert!(wal.snapshot(&engine).is_err());
        assert_eq!(engine.get("key").unwrap().data, b"old");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
