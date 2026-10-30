# distributed-cache

Cache distribué en Rust avec API HTTP, routage par consistent hashing,
réplication asynchrone, expiration TTL et persistance locale WAL + snapshot.

## Démarrer

Prérequis : Rust 1.85 ou supérieur. Python 3 permet d'exécuter le test de cluster.

```bash
cargo build --release --locked
./target/release/cache-node --node-id node-1 --data-dir ./data/node1
```

Le serveur écoute sur toutes les interfaces. `--host` désigne l'adresse **annoncée
aux pairs**, par défaut `127.0.0.1`. Dans Docker, utiliser le nom du service.
Les adresses annoncées et celles de `--peers` doivent être identiques sur tous
les nœuds : elles définissent les propriétaires des clés dans le ring.

### Cluster local à trois nœuds

Exécuter chaque commande dans un terminal distinct :

```bash
./target/release/cache-node --node-id node-1 --http-port 8080 --node-port 9090 \
  --peers 127.0.0.1:8081,127.0.0.1:8082 --data-dir ./data/node1

./target/release/cache-node --node-id node-2 --http-port 8081 --node-port 9091 \
  --peers 127.0.0.1:8080,127.0.0.1:8082 --data-dir ./data/node2

./target/release/cache-node --node-id node-3 --http-port 8082 --node-port 9092 \
  --peers 127.0.0.1:8080,127.0.0.1:8081 --data-dir ./data/node3
```

Chaque processus doit disposer de son propre répertoire de données.
Pour Docker :

```bash
docker compose up --build -d
docker compose ps
docker compose down
```

Les volumes conservent les données après `docker compose down`.

## API

Les requêtes publiques peuvent être adressées à n'importe quel nœud :

```bash
# Écrire une valeur texte ; TTL en secondes, 3600 par défaut.
curl -f -X PUT 'http://localhost:8080/v1/cache/my-key?ttl=300' -d 'my-value'

# Lire via un autre nœud.
curl -f http://localhost:8082/v1/cache/my-key

# Supprimer ; renvoie {"key":"my-key","deleted":true} si présente.
curl -f -X DELETE http://localhost:8081/v1/cache/my-key

curl -f http://localhost:8080/v1/cluster/nodes
curl -f http://localhost:8080/health
curl -f http://localhost:8080/metrics
```

Une lecture réussie renvoie :

```json
{
  "key": "my-key",
  "value": "my-value",
  "expires_at_unix": 1700000300,
  "created_at_unix": 1700000000,
  "served_by": "node-3"
}
```

`served_by` identifie le nœud qui répond au client. Une clé absente ou expirée
renvoie HTTP 404. `ttl=0` expire immédiatement. Les corps de PUT sont limités à
16 Mio (HTTP 413 au-delà). Le stockage conserve les octets ; l'API GET publique
les convertit en texte UTF-8 avec remplacement des séquences invalides.
Une erreur de persistance renvoie HTTP 500 sans appliquer la mutation en mémoire.
Les erreurs de transport vers un pair renvoient HTTP 502 ou 503.

## Garanties et fonctionnement

- **Routage** : ring SHA-256 avec 256 virtual nodes par adresse. Les noms
  `--node-id` servent à l'observabilité et peuvent être personnalisés. Tous les
  membres doivent configurer la même liste d'adresses, en incluant leur adresse
  locale implicitement. Le gossip suit la santé des nœuds ; il ne reconfigure pas
  le ring et ne déplace pas les données lors d'un changement de topologie.
- **Stockage** : DashMap, expiration à la lecture et nettoyage toutes les
  30 secondes. L'éviction LRU utilise un compteur logique d'accès. La capacité
  est contrôlée par le nettoyage périodique, elle peut être dépassée entre deux
  passages. Les compteurs d'entrées proviennent du contenu réel de la map.
- **Réplication** : le facteur inclut le primaire (`2` = primaire + une copie).
  Le propriétaire propage les écritures et suppressions en arrière-plan, y
  compris lorsqu'il reçoit une requête transférée. Les copies conservent les
  dates de création et d'expiration du primaire. Une réponse réussie garantit
  l'écriture locale du destinataire, pas l'arrivée des copies sur les répliques.
  La réplication est best effort : les opérations concurrentes peuvent arriver
  dans un ordre différent, sans quorum, consensus ni réparation automatique.
- **Circuit breaker** : ouverture après plusieurs échecs, puis essais de
  récupération après le délai configuré. Le repli vers un autre nœud ne garantit
  pas qu'il possède une copie à jour de la clé.
- **Persistance** : les mutations sont journalisées et synchronisées sur disque
  avant leur application en mémoire. Les nouveaux records conservent les dates
  absolues : un redémarrage ne renouvelle pas le TTL. Un snapshot bloque les
  mutations journalisées pendant sa capture et sa publication atomique. Son
  checkpoint identifie la génération et la position du WAL ; la reprise reste
  correcte si le processus s'arrête avant ou après le remplacement du journal.
  Les E/S de persistance des requêtes s'exécutent dans le pool bloquant de Tokio.
- **Récupération** : les records incomplets en fin de WAL sont tronqués avant
  les nouvelles écritures. Un mauvais checksum, un en-tête invalide ou un snapshot
  illisible provoque un échec explicite au démarrage. Après une erreur d'écriture
  du WAL, les mutations suivantes sont rejetées jusqu'au redémarrage.

Les anciens WAL v1 et snapshots restent lisibles. Les records SET v1 ne contiennent
pas leur date de création : leur TTL relatif est donc réinitialisé à la migration.
Le nouveau format et le nouveau protocole interne nécessitent une mise à jour de
tous les nœuds ensemble ; les anciennes versions ne peuvent pas relire le WAL v2.
Les anciens snapshots/archives ne sont pas supprimés automatiquement à la migration.
Le ring utilise désormais les adresses à la place des anciens identifiants : lors
d'une migration d'un ancien cluster, prévoir de réchauffer les clés auprès du
backend d'origine, car leurs anciens propriétaires peuvent différer.

Les routes `/internal/*` sont disponibles sur les deux ports pour que `--peers`
puisse utiliser les adresses HTTP annoncées. Ce projet ne fournit ni authentification
ni TLS : déployer le cluster sur un réseau privé avec des pairs de confiance.

## Configuration

```bash
./target/release/cache-node --help
RUST_LOG=info ./target/release/cache-node --data-dir ./data/node1
```

| Option | Défaut | Rôle |
| --- | --- | --- |
| `--host` | `127.0.0.1` | Hôte annoncé, joignable par les pairs |
| `--node-id` | `node-1` | Nom du nœud |
| `--http-port` | `8080` | Port HTTP public et protocole interne |
| `--node-port` | `9090` | Second listener du protocole interne |
| `--peers` | vide | Adresses HTTP des autres nœuds, séparées par `,` |
| `--data-dir` | `./data` | Répertoire WAL + snapshot |
| `--max-entries` | `10000000` | Seuil de nettoyage LRU |
| `--default-ttl-secs` | `3600` | TTL par défaut |
| `--replication-factor` | `2` | Nombre total de copies souhaité |
| `--virtual-nodes` | `256` | Virtual nodes par adresse |
| `--gossip-interval-secs` | `30` | Intervalle du gossip |
| `--heartbeat-interval-secs` | `5` | Intervalle des sondes de santé |
| `--circuit-breaker-threshold` | `5` | Échecs avant ouverture |
| `--circuit-breaker-timeout-secs` | `30` | Délai avant essai de récupération |
| `--snapshot-interval-secs` | `60` | Intervalle des snapshots |
| `--request-timeout-secs` | `10` | Timeout des échanges HTTP avec les pairs |

Les intervalles, capacités et facteurs doivent être strictement positifs.
Les ports public et interne doivent être différents.

## Validation

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
python3 tests/cluster_smoke.py
```

Les tests couvrent la concurrence sur le stockage, l'ordre LRU, les WAL tronqués
ou corrompus, les erreurs disque, la compatibilité v1, les snapshots concurrents,
les timeouts et l'accord des rings. Le test de cluster démarre trois processus,
vérifie le routage, les grosses valeurs, le gossip, les copies et suppressions,
les TTL, puis tue et redémarre le cluster pour vérifier la récupération.

`bench.sh [URL]` mesure le débit HTTP dans votre environnement (outils : curl,
bc, Python 3). Aucune latence ni réduction de latence n'est garantie sans mesure.
Les synchronisations disque, le volume de données et la réplication influencent
les performances.
