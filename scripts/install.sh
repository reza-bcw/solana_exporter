#!/usr/bin/env bash
set -euo pipefail

MODE="${1:-validator}"
PREFIX="${PREFIX:-/opt/solana-kpi-exporter}"
CONFIG_DIR="${CONFIG_DIR:-/etc/solana-kpi-exporter}"
SERVICE_NAME="${SERVICE_NAME:-solana-kpi-exporter}"
SYSTEMD_DIR="${SYSTEMD_DIR:-/etc/systemd/system}"

mkdir -p "$PREFIX" "$CONFIG_DIR" /var/lib/solana-kpi-exporter

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
rsync -a --delete \
  --exclude target \
  --exclude .git \
  "$PROJECT_ROOT/" "$PREFIX/"

cd "$PREFIX"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found. Install Rust first."
  exit 1
fi

bash "$PREFIX/scripts/render-config.sh" "$MODE" "$CONFIG_DIR/config.toml"

cargo build --release --manifest-path "$PREFIX/Cargo.toml"

install -m 0755 "$PREFIX/target/release/solana-kpi-exporter" /usr/local/bin/solana-kpi-exporter

sed \
  -e "s|__CONFIG__|$CONFIG_DIR/config.toml|g" \
  -e "s|__BINARY__|/usr/local/bin/solana-kpi-exporter|g" \
  "$PREFIX/systemd/solana-kpi-exporter.service" > "$SYSTEMD_DIR/$SERVICE_NAME.service"

systemctl daemon-reload
systemctl enable --now "$SERVICE_NAME"

echo "done"
systemctl status "$SERVICE_NAME" --no-pager || true
echo "metrics: curl -s http://127.0.0.1:9898/metrics | grep '^solana_' | head"
echo "config: cat $CONFIG_DIR/config.toml"
