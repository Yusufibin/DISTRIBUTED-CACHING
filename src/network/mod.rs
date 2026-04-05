pub mod http_server;
pub mod node_client;
pub mod node_server;

use crate::{
    cluster::ClusterState,
    config::Config,
    metrics::Metrics,
    persistence::wal::WriteAheadLog,
    router::SmartRouter,
    storage::engine::StorageEngine,
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
