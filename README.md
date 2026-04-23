# Solana KPI Exporter v2

Production-safer version.

## Fixes vs previous build

- Keeps the **last good snapshot** on collection failure
- Exposes `solana_kpi_scrape_success` and `solana_kpi_last_success_timestamp_unix`
- Uses `poll_interval_secs = 60` by default
- Uses `timeout_secs = 6` by default
- Uses explicit `--manifest-path` during install/build
- Includes `[workspace]` in `Cargo.toml` to avoid host root workspace pollution

## Install

```bash
bash scripts/install.sh validator
```

## Metrics endpoint

```bash
curl -s http://127.0.0.1:9898/metrics
```
