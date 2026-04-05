use clap::Parser;

#[derive(Debug, Clone, Parser)]
#[command(name = "cache-node", about = "High-performance distributed cache node")]
pub struct Config {
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    #[arg(long, default_value_t = 8080)]
    pub http_port: u16,

    #[arg(long, default_value_t = 9090)]
    pub node_port: u16,

    #[arg(long, default_value = "node-1")]
    pub node_id: String,

    #[arg(long, num_args = 0.., value_delimiter = ',')]
    pub peers: Vec<String>,

    #[arg(long, default_value_t = 10_000_000)]
    pub max_entries: u64,

    #[arg(long, default_value_t = 3600)]
    pub default_ttl_secs: u64,

    #[arg(long, default_value_t = 2)]
    pub replication_factor: usize,

    #[arg(long, default_value = "./data")]
    pub data_dir: String,

    #[arg(long, default_value_t = 256)]
    pub virtual_nodes: usize,

    #[arg(long, default_value_t = 30)]
    pub gossip_interval_secs: u64,

    #[arg(long, default_value_t = 5)]
    pub heartbeat_interval_secs: u64,

    #[arg(long, default_value_t = 5)]
    pub circuit_breaker_threshold: u64,

    #[arg(long, default_value_t = 30)]
    pub circuit_breaker_timeout_secs: u64,

    #[arg(long, default_value_t = 60)]
    pub snapshot_interval_secs: u64,

    #[arg(long, default_value_t = 10)]
    pub request_timeout_secs: u64,
}

impl Config {
    pub fn node_addr(&self) -> String {
        format!("{}:{}", self.host, self.http_port)
    }

    #[allow(dead_code)]
    pub fn node_internal_addr(&self) -> String {
        format!("{}:{}", self.host, self.node_port)
    }
}
