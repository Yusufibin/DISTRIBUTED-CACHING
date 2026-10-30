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

impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.http_port != self.node_port,
            "HTTP and internal ports must differ"
        );
        anyhow::ensure!(
            self.max_entries > 0 && self.virtual_nodes > 0 && self.replication_factor > 0,
            "capacity, virtual nodes and replication factor must be positive"
        );
        anyhow::ensure!(
            self.gossip_interval_secs > 0
                && self.heartbeat_interval_secs > 0
                && self.snapshot_interval_secs > 0
                && self.request_timeout_secs > 0
                && self.circuit_breaker_threshold > 0
                && self.circuit_breaker_timeout_secs > 0,
            "intervals, timeouts and circuit breaker threshold must be positive"
        );
        anyhow::ensure!(
            !self.host.is_empty() && self.host != "0.0.0.0" && self.host != "::",
            "--host must be an advertised hostname or address reachable by peers"
        );
        for peer in &self.peers {
            let (host, port) = peer
                .rsplit_once(':')
                .ok_or_else(|| anyhow::anyhow!("invalid peer: {peer}"))?;
            anyhow::ensure!(
                !host.is_empty() && port.parse::<u16>().is_ok_and(|port| port > 0),
                "invalid peer: {peer}"
            );
        }
        Ok(())
    }
}
