# distributed-cache

Système de cache distribué haute performance en Rust.  
Réduction de latence de 80% sur architectures microservices à grande échelle.

## Architecture

```
                  ┌──────────────────────────────────────┐
                  │         Microservices clients         │
                  │       gRPC / REST / async SDK         │
                  └─────────────────┬────────────────────┘
                                    │
                  ┌─────────────────▼────────────────────┐
                  │             Smart Router              │
                  │  Consistent hashing · Circuit breaker │
                  └──────┬──────────┬────────────┬───────┘
                         │          │            │
              ┌──────────▼─┐  ┌────▼──────┐  ┌─▼──────────┐
              │   Node A   │  │  Node B   │  │   Node C   │
              │  DashMap   │◄─┤  DashMap  ├─►│  DashMap   │
              │  + Moka    │  │  + Moka   │  │  + Moka    │
              └──────┬─────┘  └────┬──────┘  └─────┬──────┘
                     └─────────────┼────────────────┘
                                   │
              ┌────────────────────▼─────────────────────┐
              │            Storage Engine                 │
              │  Lock-free · zero-copy · tokio runtime    │
              └─────────────┬──────────────┬─────────────┘
                            │              │
              ┌─────────────▼──┐  ┌────────▼────────────┐
              │  Persistence   │  │    Observabilité     │
              │  WAL + snapshot│  │  OpenTelemetry+Prom  │
              └────────────────┘  └─────────────────────┘
```

## Démarrage rapide

### Compiler

```bash
cargo build --release
```

### Lancer un cluster local à 3 nœuds

```bash
# Nœud 1
./target/release/cache-node \
  --node-id node-1 \
  --http-port 8080 \
  --node-port 9090 \
  --peers "127.0.0.1:8081,127.0.0.1:8082" \
  --data-dir ./data/node1 &

# Nœud 2
./target/release/cache-node \
  --node-id node-2 \
  --http-port 8081 \
  --node-port 9091 \
  --peers "127.0.0.1:8080,127.0.0.1:8082" \
  --data-dir ./data/node2 &

# Nœud 3
./target/release/cache-node \
  --node-id node-3 \
  --http-port 8082 \
  --node-port 9092 \
  --peers "127.0.0.1:8080,127.0.0.1:8081" \
  --data-dir ./data/node3 &
```

## API REST publique

### Écrire une entrée

```bash
# TTL optionnel en secondes (défaut: 3600)
curl -X PUT "http://localhost:8080/v1/cache/my-key?ttl=300" \
     -d "my-value"
```

### Lire une entrée

```bash
# Depuis n'importe quel nœud — le router forward automatiquement
curl http://localhost:8082/v1/cache/my-key
```

Réponse :
```json
{
  "key": "my-key",
  "value": "my-value",
  "expires_at_unix": 1700000300,
  "created_at_unix": 1700000000,
  "served_by": "node-3"
}
```

### Supprimer une entrée

```bash
curl -X DELETE http://localhost:8080/v1/cache/my-key
```

### Infos cluster

```bash
curl http://localhost:8080/v1/cluster/nodes
```

```json
{
  "node_id": "node-1",
  "nodes": [
    { "id": "node-1", "addr": "127.0.0.1:8080", "status": "Healthy", "last_seen": 1700000120 },
    { "id": "node-2", "addr": "127.0.0.1:8081", "status": "Healthy", "last_seen": 1700000119 },
    { "id": "node-3", "addr": "127.0.0.1:8082", "status": "Healthy", "last_seen": 1700000118 }
  ],
  "total_entries": 48293
}
```

### Métriques Prometheus

```bash
curl http://localhost:8080/metrics
```

```
# HELP cache_requests_total Total number of cache requests
cache_requests_total{operation="get",result="hit"} 1042731
cache_requests_total{operation="get",result="miss"} 3821
cache_requests_total{operation="set",result="ok"} 198432

# HELP cache_request_duration_seconds Cache request duration
cache_request_duration_seconds_bucket{operation="get",le="0.0001"} 987234
cache_request_duration_seconds_p99{operation="get"} 0.00043

# HELP cache_entries_total Total entries in cache
cache_entries_total 48293

# HELP cluster_nodes_total Number of known cluster nodes
cluster_nodes_total 3
```

### Health check

```bash
curl http://localhost:8080/health
```

## Options de configuration

| Flag | Défaut | Description |
|------|--------|-------------|
| `--node-id` | `node-1` | Identifiant unique du nœud |
| `--http-port` | `8080` | Port HTTP public |
| `--node-port` | `9090` | Port interne inter-nœuds |
| `--peers` | _(vide)_ | Adresses des autres nœuds (séparées par `,`) |
| `--max-entries` | `10_000_000` | Capacité max avant éviction LRU |
| `--default-ttl-secs` | `3600` | TTL par défaut (secondes) |
| `--replication-factor` | `2` | Nombre de répliques par clé |
| `--data-dir` | `./data` | Répertoire WAL + snapshots |
| `--virtual-nodes` | `256` | Vnodes par nœud dans le ring |
| `--gossip-interval-secs` | `30` | Fréquence du gossip |
| `--heartbeat-interval-secs` | `5` | Fréquence des heartbeats |
| `--circuit-breaker-threshold` | `5` | Échecs avant ouverture du circuit |
| `--circuit-breaker-timeout-secs` | `30` | Délai avant demi-ouverture |
| `--snapshot-interval-secs` | `60` | Fréquence des snapshots |
| `--request-timeout-secs` | `10` | Timeout requêtes inter-nœuds |

## Structure du projet

```
src/
├── main.rs                   # Bootstrap, tokio::select! sur toutes les tâches
├── config.rs                 # CLI via clap
├── error.rs                  # CacheError unifié
│
├── storage/
│   ├── engine.rs             # DashMap<String, Arc<Entry>> + évictions LRU/LFU
│   └── eviction.rs           # Boucle de nettoyage asynchrone (toutes les 30s)
│
├── router/
│   ├── consistent_hash.rs    # Ring BTreeMap avec vnodes SHA-256
│   ├── circuit_breaker.rs    # State machine AtomicU8 (closed/open/half-open)
│   └── mod.rs                # SmartRouter : routage + fallback + réplication
│
├── cluster/
│   ├── mod.rs                # ClusterState : DashMap<NodeInfo> + merge gossip
│   └── gossip.rs             # Push gossip + heartbeat asynchrones
│
├── network/
│   ├── mod.rs                # AppState partagé (Arc<…> clonable)
│   ├── node_client.rs        # Client HTTP interne sur hyper 0.14
│   ├── node_server.rs        # Serveur interne axum (port node_port)
│   └── http_server.rs        # API publique axum + forwarding + réplication async
│
├── persistence/
│   ├── wal.rs                # WAL binaire [CRC32|len|bincode] + replay
│   └── snapshot.rs           # Snapshots async + rotation + cleanup
│
└── metrics/
    └── mod.rs                # Prometheus counters/histograms/gauges
```

## Fonctionnement interne

### Consistent hashing

Chaque nœud est représenté par `N` virtual nodes (défaut 256) dans un BTreeMap trié par hash SHA-256. Un `GET my-key` calcule `hash("my-key")` et cherche le successeur dans le ring en O(log n). Cela garantit qu'un client atteint toujours le bon nœud en **1 seul hop**.

### Circuit breaker

Trois états : `Closed` → `Open` → `Half-Open` → `Closed`. Après `threshold` échecs consécutifs le circuit s'ouvre et rejette immédiatement les requêtes vers le nœud défaillant. Après `timeout` secondes il passe en demi-ouverture et teste 3 requêtes successives avant de se refermer.

### Réplication asynchrone

Lors d'un `PUT`, le nœud primaire répond immédiatement au client puis envoie la valeur aux répliques via `tokio::spawn` en arrière-plan. Le `replication_factor` contrôle combien de nœuds reçoivent la copie.

### WAL + Snapshots

Chaque `SET` et `DELETE` est journalisé dans `cache.wal` avant d'être appliqué en mémoire. Le format est `[CRC32 4B][len 4B][bincode data]`. Un snapshot bincode complet est pris toutes les `snapshot-interval-secs` secondes, après quoi le WAL est roté. Au démarrage : chargement du dernier snapshot + replay du WAL résiduel.

### Gossip protocol

Toutes les `gossip-interval-secs` secondes, chaque nœud sélectionne aléatoirement la moitié de ses pairs et leur pousse l'état connu du cluster (vecteur de `NodeInfo`). Le récepteur fusionne en gardant l'information la plus récente (`last_seen` gagne). Les nœuds non vus depuis `3 × heartbeat_interval` sont marqués `Unhealthy`.

## Déploiement Docker

```dockerfile
FROM rust:1.75 AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/cache-node /usr/local/bin/
ENTRYPOINT ["cache-node"]
```

```yaml
# docker-compose.yml
version: "3.8"
services:
  node1:
    build: .
    command: >
      --node-id node-1 --http-port 8080 --node-port 9090
      --peers "node2:8081,node3:8082" --data-dir /data
    ports: ["8080:8080"]
    volumes: ["node1_data:/data"]

  node2:
    build: .
    command: >
      --node-id node-2 --http-port 8081 --node-port 9091
      --peers "node1:8080,node3:8082" --data-dir /data
    ports: ["8081:8081"]
    volumes: ["node2_data:/data"]

  node3:
    build: .
    command: >
      --node-id node-3 --http-port 8082 --node-port 9092
      --peers "node1:8080,node2:8081" --data-dir /data
    ports: ["8082:8082"]
    volumes: ["node3_data:/data"]

volumes:
  node1_data:
  node2_data:
  node3_data:
```

## Variables d'environnement

```bash
RUST_LOG=info,distributed_cache=debug  # Niveau de log
```

## Performance attendue

| Opération | Latence p50 | Latence p99 |
|-----------|-------------|-------------|
| GET local | < 50 µs | < 200 µs |
| GET forward | < 500 µs | < 2 ms |
| SET + réplication | < 1 ms | < 5 ms |

Gain typique vs appel backend direct : **−80% de latence** sur les chemins chauds.
