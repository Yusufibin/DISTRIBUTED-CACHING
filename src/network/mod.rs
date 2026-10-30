pub mod http_server;
pub mod node_client;
pub mod node_server;

use crate::{
    cluster::ClusterState, config::Config, metrics::Metrics, persistence::wal::WriteAheadLog,
    router::SmartRouter, storage::engine::StorageEngine,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<StorageEngine>,
    pub router: Arc<SmartRouter>,
    pub cluster: Arc<ClusterState>,
    pub wal: Arc<WriteAheadLog>,
    pub metrics: Arc<Metrics>,
    pub config: Config,
}

impl AppState {
    pub async fn set(
        &self,
        key: String,
        value: crate::storage::engine::CacheValue,
    ) -> crate::error::Result<()> {
        let storage = self.storage.clone();
        let wal = self.wal.clone();
        tokio::task::spawn_blocking(move || wal.set(&storage, key, value))
            .await
            .map_err(|error| crate::error::CacheError::Upstream(error.to_string()))?
    }

    pub async fn delete(&self, key: String) -> crate::error::Result<bool> {
        let storage = self.storage.clone();
        let wal = self.wal.clone();
        tokio::task::spawn_blocking(move || wal.delete(&storage, &key))
            .await
            .map_err(|error| crate::error::CacheError::Upstream(error.to_string()))?
    }
}
