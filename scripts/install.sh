#!/usr/bin/env bash
set -euo pipefail
MODE="${1:-validator}"
PREFIX="${PREFIX:-/opt/solana-kpi-exporter}"
CONFIG_DIR="${CONFIG_DIR:-/etc/solana-kpi-exporter}"
SERVICE_NAME="${SERVICE_NAME:-solana-kpi-exporter}"
SYSTEMD_DIR="${SYSTEMD_DIR:-/etc/systemd/system}"
echo "==> creating directories"
mkdir -p "$PREFIX" "$CONFIG_DIR" /var/lib/solana-kpi-exporter
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
echo "==> copying project files"
rsync -a --delete --exclude target --exclude .git "$PROJECT_ROOT/" "$PREFIX/"
cd "$PREFIX"
command -v cargo >/dev/null 2>&1 || { echo "cargo not found. Install Rust first."; exit 1; }
echo "==> rendering config for mode=$MODE"
bash "$PREFIX/scripts/render-config.sh" "$MODE" "$CONFIG_DIR/config.toml"
echo "==> building release binary"
cargo build --release
echo "==> installing binary"
install -m 0755 "$PREFIX/target/release/solana-kpi-exporter" /usr/local/bin/solana-kpi-exporter
echo "==> installing systemd unit"
sed -e "s|__CONFIG__|$CONFIG_DIR/config.toml|g" -e "s|__BINARY__|/usr/local/bin/solana-kpi-exporter|g" "$PREFIX/systemd/solana-kpi-exporter.service" > "$SYSTEMD_DIR/$SERVICE_NAME.service"
systemctl daemon-reload
systemctl enable --now "$SERVICE_NAME"
echo "==> done"
systemctl status "$SERVICE_NAME" --no-pager || true
echo "metrics: curl -s http://127.0.0.1:9898/metrics | grep '^solana_' | head"
echo "config: cat $CONFIG_DIR/config.toml"
