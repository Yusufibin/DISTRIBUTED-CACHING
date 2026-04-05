use crate::{
    error::Result,
    storage::engine::{CacheValue, StorageEngine},
};
use bincode;
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
const WAL_VERSION: u8 = 1;

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
}

#[allow(dead_code)]
struct WalFile {
    file: File,
    path: PathBuf,
}

impl WalFile {
    fn open_or_create(path: &Path) -> std::io::Result<Self> {
        let exists = path.exists();
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)?;

        if !exists {
            file.write_all(WAL_MAGIC)?;
            file.write_all(&[WAL_VERSION])?;
            file.flush()?;
        }

        Ok(WalFile {
            file,
            path: path.to_path_buf(),
        })
    }

    fn write_entry(&mut self, entry: &WalEntry) -> std::io::Result<()> {
        let data = bincode::serialize(entry).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;

        let mut crc = Crc32Hasher::new();
        crc.update(&data);
        let checksum = crc.finalize();

        let len = data.len() as u32;
        self.file.write_all(&checksum.to_le_bytes())?;
        self.file.write_all(&len.to_le_bytes())?;
        self.file.write_all(&data)?;
        self.file.flush()
    }

    fn read_all_entries(&mut self) -> std::io::Result<Vec<WalEntry>> {
        self.file.seek(SeekFrom::Start(5))?;
        let mut entries = Vec::new();

        loop {
            let mut crc_buf = [0u8; 4];
            match self.file.read_exact(&mut crc_buf) {
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e),
                Ok(_) => {}
            }

            let mut len_buf = [0u8; 4];
            self.file.read_exact(&mut len_buf)?;
            let len = u32::from_le_bytes(len_buf) as usize;

            let mut data = vec![0u8; len];
            self.file.read_exact(&mut data)?;

            let mut crc = Crc32Hasher::new();
            crc.update(&data);
            let computed = crc.finalize();
            let stored = u32::from_le_bytes(crc_buf);

            if computed != stored {
                tracing::error!("WAL corruption detected, stopping replay at this point");
                break;
            }

            match bincode::deserialize::<WalEntry>(&data) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    tracing::warn!("Failed to deserialize WAL entry: {}", e);
                    break;
                }
            }
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
        Ok(WriteAheadLog {
            inner: Mutex::new(wal_file),
            path,
        })
    }

    pub fn log_set(&self, key: &str, value: &[u8], ttl_secs: Option<u64>) -> Result<()> {
        let entry = WalEntry::Set {
            key: key.to_string(),
            value: value.to_vec(),
            ttl_secs,
        };
        self.inner.lock().write_entry(&entry)?;
        Ok(())
    }

    pub fn log_delete(&self, key: &str) -> Result<()> {
        let entry = WalEntry::Delete {
            key: key.to_string(),
        };
        self.inner.lock().write_entry(&entry)?;
        Ok(())
    }

    pub fn log_checkpoint(&self, timestamp: u64) -> Result<()> {
        let entry = WalEntry::Checkpoint { timestamp };
        self.inner.lock().write_entry(&entry)?;
        Ok(())
    }

    pub async fn replay(&self, engine: &Arc<StorageEngine>) -> Result<()> {
        let entries = self.inner.lock().read_all_entries()?;
        let count = entries.len();

        for entry in entries {
            match entry {
                WalEntry::Set { key, value, ttl_secs } => {
                    let ttl = ttl_secs.map(|s| std::time::Duration::from_secs(s));
                    let cache_val = CacheValue::new(value, ttl);
                    engine.set(key, cache_val);
                }
                WalEntry::Delete { key } => {
                    engine.delete(&key);
                }
                WalEntry::Checkpoint { .. } => {}
            }
        }

        tracing::info!("WAL replay: applied {} entries", count);
        Ok(())
    }

    pub fn rotate(&self) -> Result<()> {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let archive_path = self.path.with_extension(format!("wal.{}", ts));

        std::fs::rename(&self.path, &archive_path)?;
        let new_file = WalFile::open_or_create(&self.path)?;
        *self.inner.lock() = new_file;

        tracing::info!("WAL rotated -> {:?}", archive_path);
        Ok(())
    }
}
