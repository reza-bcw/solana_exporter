# Solana KPI Exporter

Rust Prometheus exporter for Solana validators and RPC nodes.

## What it exposes

### Toko KPI metrics
- Uptime target and rolling uptime ratio
- Voting effectiveness percentage and target
- Skip rate percentage and target

### Validator metrics
- vote credits earned in current epoch
- last vote / root slot / lag
- delinquent status
- activated stake
- commission
- leader slots / produced / skipped

### RPC / node metrics
- health
- version
- slot / block height / epoch
- sync lag vs reference RPC
- max shred insert slot / shred gap
- snapshot slots / snapshot lag
- identity balance / vote balance

## Quick install

```bash
bash scripts/install.sh validator
```

Or for RPC-only:

```bash
bash scripts/install.sh rpc
```

## Render config only

```bash
bash scripts/render-config.sh validator /etc/solana-kpi-exporter/config.toml
bash scripts/render-config.sh rpc /etc/solana-kpi-exporter/config.toml
```
