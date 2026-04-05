#!/usr/bin/env bash
set -e

BASE_URL="${1:-http://localhost:8080}"
KEYS=1000
CONCURRENCY=50

echo "=== distributed-cache benchmark ==="
echo "Target: $BASE_URL"
echo "Keys:   $KEYS"
echo ""

command -v curl  >/dev/null 2>&1 || { echo "curl required"; exit 1; }
command -v bc    >/dev/null 2>&1 || { echo "bc required"; exit 1; }

echo "--- Health check ---"
curl -sf "$BASE_URL/health" | python3 -m json.tool 2>/dev/null || curl -sf "$BASE_URL/health"
echo ""

echo "--- Writing $KEYS keys (concurrency $CONCURRENCY) ---"
WRITE_START=$(date +%s%3N)

seq 1 "$KEYS" | xargs -P "$CONCURRENCY" -I{} \
  curl -sf -X PUT "$BASE_URL/v1/cache/bench-key-{}?ttl=300" -d "value-{}" -o /dev/null

WRITE_END=$(date +%s%3N)
WRITE_MS=$(( WRITE_END - WRITE_START ))
WRITE_OPS=$(echo "scale=0; $KEYS * 1000 / $WRITE_MS" | bc)
echo "  Completed in ${WRITE_MS}ms — ~${WRITE_OPS} ops/s"
echo ""

echo "--- Reading $KEYS keys (concurrency $CONCURRENCY) ---"
READ_START=$(date +%s%3N)

seq 1 "$KEYS" | xargs -P "$CONCURRENCY" -I{} \
  curl -sf "$BASE_URL/v1/cache/bench-key-{}" -o /dev/null

READ_END=$(date +%s%3N)
READ_MS=$(( READ_END - READ_START ))
READ_OPS=$(echo "scale=0; $KEYS * 1000 / $READ_MS" | bc)
echo "  Completed in ${READ_MS}ms — ~${READ_OPS} ops/s"
echo ""

echo "--- Cluster state ---"
curl -sf "$BASE_URL/v1/cluster/nodes" | python3 -m json.tool 2>/dev/null \
  || curl -sf "$BASE_URL/v1/cluster/nodes"
echo ""

echo "--- Key metrics ---"
curl -sf "$BASE_URL/metrics" | grep -E '^(cache_requests_total|cache_entries_total|cache_request_duration)'
echo ""

echo "=== Done ==="
