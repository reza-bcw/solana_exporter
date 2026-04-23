#!/usr/bin/env bash
set -euo pipefail

MODE="${1:-validator}"
OUT="${2:-/etc/solana-kpi-exporter/config.toml}"

mkdir -p "$(dirname "$OUT")"

TARGET_URL="${TARGET_URL:-http://45.250.254.83/rpc}"
REFERENCE_URL="${REFERENCE_URL:-http://45.250.254.83/rpc}"
COMMITMENT="${COMMITMENT:-finalized}"
TIMEOUT_SECS="${TIMEOUT_SECS:-4}"
NODE_PUBKEY="${NODE_PUBKEY:-BaDhUB1eWfunwD21Tu3WywyYQ9wZx5hS9WXeHHNGZUPy}"
VOTE_PUBKEY="${VOTE_PUBKEY:-CBSrVMzHqnjb1td6diYaqy2Nq1GotkKQBB6i5eaR1ZyR}"
LISTEN_ADDR="${LISTEN_ADDR:-0.0.0.0:9898}"
POLL_INTERVAL_SECS="${POLL_INTERVAL_SECS:-120}"
STATE_PATH="${STATE_PATH:-/var/lib/solana-kpi-exporter/state.json}"
LOG_LEVEL="${LOG_LEVEL:-info}"
WINDOW_HOURS="${WINDOW_HOURS:-720}"
MAX_SYNC_LAG_SLOTS="${MAX_SYNC_LAG_SLOTS:-150}"
MAX_LAST_VOTE_LAG_SLOTS="${MAX_LAST_VOTE_LAG_SLOTS:-8}"
MAX_ROOT_LAG_SLOTS="${MAX_ROOT_LAG_SLOTS:-64}"
UPTIME_TARGET_PCT="${UPTIME_TARGET_PCT:-99.95}"
VOTING_EFFECTIVENESS_TARGET_PCT="${VOTING_EFFECTIVENESS_TARGET_PCT:-99.8}"
SKIP_RATE_TARGET_PCT="${SKIP_RATE_TARGET_PCT:-0.0}"

if [[ "$MODE" == "validator" ]]; then
  REQUIRE_NOT_DELINQUENT="true"
  ENABLE_VOTE_METRICS="true"
  ENABLE_SKIP_RATE="true"
else
  REQUIRE_NOT_DELINQUENT="false"
  ENABLE_VOTE_METRICS="false"
  ENABLE_SKIP_RATE="false"
  VOTE_PUBKEY=""
fi

cat > "$OUT" <<EOF
mode = "$MODE"

[rpc]
target_url = "$TARGET_URL"
reference_url = "$REFERENCE_URL"
commitment = "$COMMITMENT"
timeout_secs = $TIMEOUT_SECS

[identity]
node_pubkey = "$NODE_PUBKEY"
vote_pubkey = "$VOTE_PUBKEY"

[exporter]
listen_addr = "$LISTEN_ADDR"
poll_interval_secs = $POLL_INTERVAL_SECS
state_path = "$STATE_PATH"
log_level = "$LOG_LEVEL"

[uptime]
window_hours = $WINDOW_HOURS
max_sync_lag_slots = $MAX_SYNC_LAG_SLOTS
max_last_vote_lag_slots = $MAX_LAST_VOTE_LAG_SLOTS
max_root_lag_slots = $MAX_ROOT_LAG_SLOTS
require_health_ok = true
require_not_delinquent = $REQUIRE_NOT_DELINQUENT

[features]
enable_vote_metrics = $ENABLE_VOTE_METRICS
enable_skip_rate = $ENABLE_SKIP_RATE
enable_balance_metrics = true
enable_snapshot_metrics = true
enable_sync_metrics = true
enable_rpc_status_metrics = true

[slo]
uptime_target_pct = $UPTIME_TARGET_PCT
voting_effectiveness_target_pct = $VOTING_EFFECTIVENESS_TARGET_PCT
skip_rate_target_pct = $SKIP_RATE_TARGET_PCT
EOF

echo "Wrote $OUT for mode=$MODE"