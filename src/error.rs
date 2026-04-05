use thiserror::Error;

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum CacheError {
    #[error("key not found: {0}")]
    KeyNotFound(String),
    #[error("node unavailable: {0}")]
    NodeUnavailable(String),
    #[error("circuit open for node: {0}")]
    CircuitOpen(String),
    #[error("no node available for key: {0}")]
    NoNodeAvailable(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("persistence error: {0}")]
    Persistence(#[from] std::io::Error),
    #[error("cluster error: {0}")]
    Cluster(String),
    #[error("invalid entry: {0}")]
    InvalidEntry(String),
    #[error("upstream error: {0}")]
    Upstream(String),
}

pub type Result<T> = std::result::Result<T, CacheError>;

impl From<bincode::Error> for CacheError {
    fn from(e: bincode::Error) -> Self {
        CacheError::Serialization(e.to_string())
    }
}


