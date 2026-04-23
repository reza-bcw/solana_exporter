use anyhow::{Context, Result};
use axum::{extract::State, response::IntoResponse, routing::get, Router};
use chrono::{DateTime, Duration, Utc};
use prometheus_client::{
    encoding::EncodeLabelSet,
    encoding::text::encode,
    metrics::{family::Family, gauge::Gauge},
    registry::Registry,
};
use reqwest::Client;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::HashMap, fs, net::SocketAddr, path::Path, sync::Arc};
use tokio::{
    sync::RwLock,
    time::{sleep, Duration as TokioDuration},
};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Validator,
    Rpc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Config {
    mode: Mode,
    rpc: RpcConfig,
    identity: IdentityConfig,
    exporter: ExporterConfig,
    uptime: UptimeConfig,
    features: FeatureConfig,
    slo: SloConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RpcConfig {
    target_url: String,
    reference_url: String,
    commitment: String,
    timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IdentityConfig {
    node_pubkey: String,
    vote_pubkey: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExporterConfig {
    listen_addr: String,
    poll_interval_secs: u64,
    state_path: String,
    log_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UptimeConfig {
    window_hours: i64,
    max_sync_lag_slots: u64,
    max_last_vote_lag_slots: u64,
    max_root_lag_slots: u64,
    require_health_ok: bool,
    require_not_delinquent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FeatureConfig {
    enable_vote_metrics: bool,
    enable_skip_rate: bool,
    enable_balance_metrics: bool,
    enable_snapshot_metrics: bool,
    enable_sync_metrics: bool,
    enable_rpc_status_metrics: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SloConfig {
    uptime_target_pct: f64,
    voting_effectiveness_target_pct: f64,
    skip_rate_target_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PersistedState {
    samples: Vec<UptimeSample>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UptimeSample {
    ts: DateTime<Utc>,
    up: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Snapshot {
    scrape_success: f64,
    scrape_errors_total: f64,
    scrape_duration_ms: f64,
    scrape_timestamp_unix: f64,
    last_success_timestamp_unix: f64,

    rpc_health: f64,
    uptime_ratio: f64,
    uptime_target: f64,
    uptime_slo_met: f64,

    solana_version: String,
    absolute_slot: f64,
    reference_slot: f64,
    sync_lag_slots: f64,
    block_height: f64,
    epoch: f64,
    slot_index: f64,
    slots_in_epoch: f64,
    max_shred_insert_slot: f64,
    shred_gap_slots: f64,
    highest_snapshot_full: f64,
    highest_snapshot_incremental: f64,
    snapshot_lag_slots: f64,

    validator_present: f64,
    delinquent: f64,
    activated_stake_sol: f64,
    commission_pct: f64,
    credits_earned_current_epoch: f64,
    last_vote: f64,
    root_slot: f64,
    last_vote_lag_slots: f64,
    root_lag_slots: f64,

    leader_slots_current_epoch: f64,
    produced_blocks_current_epoch: f64,
    skipped_slots_current_epoch: f64,
    skip_rate_pct: f64,
    skip_rate_target: f64,
    skip_rate_slo_met: f64,

    cluster_confirmed_blocks_current_epoch: f64,
    voting_effectiveness_pct: f64,
    voting_effectiveness_target: f64,
    voting_effectiveness_slo_met: f64,

    identity_balance_sol: f64,
    vote_balance_sol: f64,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            scrape_success: 0.0,
            scrape_errors_total: 0.0,
            scrape_duration_ms: 0.0,
            scrape_timestamp_unix: 0.0,
            last_success_timestamp_unix: 0.0,
            rpc_health: 0.0,
            uptime_ratio: 0.0,
            uptime_target: 99.95,
            uptime_slo_met: 0.0,
            solana_version: String::new(),
            absolute_slot: 0.0,
            reference_slot: 0.0,
            sync_lag_slots: 0.0,
            block_height: 0.0,
            epoch: 0.0,
            slot_index: 0.0,
            slots_in_epoch: 0.0,
            max_shred_insert_slot: 0.0,
            shred_gap_slots: 0.0,
            highest_snapshot_full: 0.0,
            highest_snapshot_incremental: 0.0,
            snapshot_lag_slots: 0.0,
            validator_present: 0.0,
            delinquent: 0.0,
            activated_stake_sol: 0.0,
            commission_pct: 0.0,
            credits_earned_current_epoch: 0.0,
            last_vote: 0.0,
            root_slot: 0.0,
            last_vote_lag_slots: 0.0,
            root_lag_slots: 0.0,
            leader_slots_current_epoch: 0.0,
            produced_blocks_current_epoch: 0.0,
            skipped_slots_current_epoch: 0.0,
            skip_rate_pct: 0.0,
            skip_rate_target: 0.0,
            skip_rate_slo_met: 0.0,
            cluster_confirmed_blocks_current_epoch: 0.0,
            voting_effectiveness_pct: 0.0,
            voting_effectiveness_target: 99.8,
            voting_effectiveness_slo_met: 0.0,
            identity_balance_sol: 0.0,
            vote_balance_sol: 0.0,
        }
    }
}

#[derive(Clone)]
struct AppState {
    registry: Arc<RwLock<Registry>>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, EncodeLabelSet)]
struct EmptyLabels {}

#[derive(Debug, Clone, Hash, PartialEq, Eq, EncodeLabelSet)]
struct NodeLabels {
    node_pubkey: String,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, EncodeLabelSet)]
struct VoteLabels {
    vote_pubkey: String,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, EncodeLabelSet)]
struct VersionLabels {
    version: String,
}

#[derive(Clone)]
struct Metrics {
    rpc_health: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    uptime_ratio: Family<NodeLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    uptime_target: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    uptime_slo_met: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    voting_effectiveness_pct: Family<VoteLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    voting_effectiveness_target: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    voting_effectiveness_slo_met: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    vote_credits_earned_current_epoch: Family<VoteLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    skip_rate_pct: Family<NodeLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    skip_rate_target: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    skip_rate_slo_met: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    leader_slots_current_epoch: Family<NodeLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    produced_blocks_current_epoch: Family<NodeLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    skipped_slots_current_epoch: Family<NodeLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    absolute_slot: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    reference_slot: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    sync_lag_slots: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    block_height: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    epoch: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    slot_index: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    slots_in_epoch: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    max_shred_insert_slot: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    shred_gap_slots: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    highest_snapshot_full: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    highest_snapshot_incremental: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    snapshot_lag_slots: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    validator_delinquent: Family<NodeLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    validator_activated_stake_sol: Family<NodeLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    validator_commission_pct: Family<NodeLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    validator_last_vote: Family<NodeLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    validator_last_vote_lag_slots: Family<NodeLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    validator_root_slot: Family<NodeLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    validator_root_lag_slots: Family<NodeLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    cluster_confirmed_blocks_current_epoch: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    identity_balance_sol: Family<NodeLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    vote_balance_sol: Family<VoteLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    version_info: Family<VersionLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    scrape_success: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    scrape_errors_total: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    scrape_duration_ms: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    scrape_timestamp_unix: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
    last_success_timestamp_unix: Family<EmptyLabels, Gauge<f64, std::sync::atomic::AtomicU64>>,
}

impl Metrics {
    fn new(registry: &mut Registry) -> Self {
        let m = Self {
            rpc_health: Family::default(),
            uptime_ratio: Family::default(),
            uptime_target: Family::default(),
            uptime_slo_met: Family::default(),
            voting_effectiveness_pct: Family::default(),
            voting_effectiveness_target: Family::default(),
            voting_effectiveness_slo_met: Family::default(),
            vote_credits_earned_current_epoch: Family::default(),
            skip_rate_pct: Family::default(),
            skip_rate_target: Family::default(),
            skip_rate_slo_met: Family::default(),
            leader_slots_current_epoch: Family::default(),
            produced_blocks_current_epoch: Family::default(),
            skipped_slots_current_epoch: Family::default(),
            absolute_slot: Family::default(),
            reference_slot: Family::default(),
            sync_lag_slots: Family::default(),
            block_height: Family::default(),
            epoch: Family::default(),
            slot_index: Family::default(),
            slots_in_epoch: Family::default(),
            max_shred_insert_slot: Family::default(),
            shred_gap_slots: Family::default(),
            highest_snapshot_full: Family::default(),
            highest_snapshot_incremental: Family::default(),
            snapshot_lag_slots: Family::default(),
            validator_delinquent: Family::default(),
            validator_activated_stake_sol: Family::default(),
            validator_commission_pct: Family::default(),
            validator_last_vote: Family::default(),
            validator_last_vote_lag_slots: Family::default(),
            validator_root_slot: Family::default(),
            validator_root_lag_slots: Family::default(),
            cluster_confirmed_blocks_current_epoch: Family::default(),
            identity_balance_sol: Family::default(),
            vote_balance_sol: Family::default(),
            version_info: Family::default(),
            scrape_success: Family::default(),
            scrape_errors_total: Family::default(),
            scrape_duration_ms: Family::default(),
            scrape_timestamp_unix: Family::default(),
            last_success_timestamp_unix: Family::default(),
        };

        reg(registry, "solana_rpc_health", "1 if getHealth is ok.", m.rpc_health.clone());
        reg(registry, "solana_kpi_uptime_ratio", "Sliding-window uptime ratio percentage.", m.uptime_ratio.clone());
        reg(registry, "solana_kpi_uptime_target", "Configured uptime target percentage.", m.uptime_target.clone());
        reg(registry, "solana_kpi_uptime_slo_met", "1 if uptime ratio meets target.", m.uptime_slo_met.clone());
        reg(registry, "solana_kpi_voting_effectiveness_pct", "Voting effectiveness percentage.", m.voting_effectiveness_pct.clone());
        reg(registry, "solana_kpi_voting_effectiveness_target", "Configured voting effectiveness target percentage.", m.voting_effectiveness_target.clone());
        reg(registry, "solana_kpi_voting_effectiveness_slo_met", "1 if voting effectiveness meets target.", m.voting_effectiveness_slo_met.clone());
        reg(registry, "solana_kpi_vote_credits_earned_current_epoch", "Vote credits earned in current epoch.", m.vote_credits_earned_current_epoch.clone());
        reg(registry, "solana_kpi_skip_rate_pct", "Skip rate percentage in current epoch.", m.skip_rate_pct.clone());
        reg(registry, "solana_kpi_skip_rate_target", "Configured skip rate target percentage.", m.skip_rate_target.clone());
        reg(registry, "solana_kpi_skip_rate_slo_met", "1 if skip rate meets target.", m.skip_rate_slo_met.clone());
        reg(registry, "solana_kpi_leader_slots_current_epoch", "Leader slots in current epoch.", m.leader_slots_current_epoch.clone());
        reg(registry, "solana_kpi_produced_blocks_current_epoch", "Produced blocks in current epoch.", m.produced_blocks_current_epoch.clone());
        reg(registry, "solana_kpi_skipped_slots_current_epoch", "Skipped slots in current epoch.", m.skipped_slots_current_epoch.clone());
        reg(registry, "solana_rpc_absolute_slot", "Local absolute slot.", m.absolute_slot.clone());
        reg(registry, "solana_rpc_reference_slot", "Reference RPC slot.", m.reference_slot.clone());
        reg(registry, "solana_rpc_sync_lag_slots", "Reference slot minus local slot.", m.sync_lag_slots.clone());
        reg(registry, "solana_rpc_block_height", "Current block height.", m.block_height.clone());
        reg(registry, "solana_rpc_epoch", "Current epoch.", m.epoch.clone());
        reg(registry, "solana_rpc_slot_index", "Current slot index in epoch.", m.slot_index.clone());
        reg(registry, "solana_rpc_slots_in_epoch", "Slots in current epoch.", m.slots_in_epoch.clone());
        reg(registry, "solana_node_max_shred_insert_slot", "Max shred insert slot.", m.max_shred_insert_slot.clone());
        reg(registry, "solana_node_shred_gap_slots", "Max shred insert slot minus absolute slot.", m.shred_gap_slots.clone());
        reg(registry, "solana_node_highest_snapshot_full", "Highest full snapshot slot.", m.highest_snapshot_full.clone());
        reg(registry, "solana_node_highest_snapshot_incremental", "Highest incremental snapshot slot.", m.highest_snapshot_incremental.clone());
        reg(registry, "solana_node_snapshot_lag_slots", "Absolute slot minus incremental snapshot slot.", m.snapshot_lag_slots.clone());
        reg(registry, "solana_validator_delinquent", "1 if validator is delinquent.", m.validator_delinquent.clone());
        reg(registry, "solana_validator_activated_stake_sol", "Activated stake in SOL.", m.validator_activated_stake_sol.clone());
        reg(registry, "solana_validator_commission_pct", "Validator commission percentage.", m.validator_commission_pct.clone());
        reg(registry, "solana_validator_last_vote", "Validator last vote slot.", m.validator_last_vote.clone());
        reg(registry, "solana_validator_last_vote_lag_slots", "Absolute slot minus last vote.", m.validator_last_vote_lag_slots.clone());
        reg(registry, "solana_validator_root_slot", "Validator root slot.", m.validator_root_slot.clone());
        reg(registry, "solana_validator_root_lag_slots", "Absolute slot minus root slot.", m.validator_root_lag_slots.clone());
        reg(registry, "solana_kpi_cluster_confirmed_blocks_current_epoch", "Cluster confirmed blocks in current epoch.", m.cluster_confirmed_blocks_current_epoch.clone());
        reg(registry, "solana_node_identity_balance_sol", "Identity account balance in SOL.", m.identity_balance_sol.clone());
        reg(registry, "solana_node_vote_balance_sol", "Vote account balance in SOL.", m.vote_balance_sol.clone());
        reg(registry, "solana_rpc_version_info", "Version info as a label; metric value always 1.", m.version_info.clone());
        reg(registry, "solana_kpi_scrape_success", "1 if last scrape succeeded.", m.scrape_success.clone());
        reg(registry, "solana_kpi_scrape_errors_total", "Collection failures since process start.", m.scrape_errors_total.clone());
        reg(registry, "solana_kpi_scrape_duration_ms", "Last scrape duration in milliseconds.", m.scrape_duration_ms.clone());
        reg(registry, "solana_kpi_scrape_timestamp_unix", "Last scrape unix timestamp.", m.scrape_timestamp_unix.clone());
        reg(registry, "solana_kpi_last_success_timestamp_unix", "Last successful collection unix timestamp.", m.last_success_timestamp_unix.clone());
        m
    }

    fn update(&self, cfg: &Config, s: &Snapshot) {
        let empty = EmptyLabels {};
        let node = NodeLabels { node_pubkey: cfg.identity.node_pubkey.clone() };
        let vote = VoteLabels { vote_pubkey: cfg.identity.vote_pubkey.clone().unwrap_or_else(|| "none".into()) };
        let ver = VersionLabels { version: if s.solana_version.is_empty() { "unknown".into() } else { s.solana_version.clone() } };

        self.rpc_health.get_or_create(&empty).set(s.rpc_health);
        self.uptime_ratio.get_or_create(&node).set(s.uptime_ratio);
        self.uptime_target.get_or_create(&empty).set(s.uptime_target);
        self.uptime_slo_met.get_or_create(&empty).set(s.uptime_slo_met);
        self.voting_effectiveness_pct.get_or_create(&vote).set(s.voting_effectiveness_pct);
        self.voting_effectiveness_target.get_or_create(&empty).set(s.voting_effectiveness_target);
        self.voting_effectiveness_slo_met.get_or_create(&empty).set(s.voting_effectiveness_slo_met);
        self.vote_credits_earned_current_epoch.get_or_create(&vote).set(s.credits_earned_current_epoch);
        self.skip_rate_pct.get_or_create(&node).set(s.skip_rate_pct);
        self.skip_rate_target.get_or_create(&empty).set(s.skip_rate_target);
        self.skip_rate_slo_met.get_or_create(&empty).set(s.skip_rate_slo_met);
        self.leader_slots_current_epoch.get_or_create(&node).set(s.leader_slots_current_epoch);
        self.produced_blocks_current_epoch.get_or_create(&node).set(s.produced_blocks_current_epoch);
        self.skipped_slots_current_epoch.get_or_create(&node).set(s.skipped_slots_current_epoch);
        self.absolute_slot.get_or_create(&empty).set(s.absolute_slot);
        self.reference_slot.get_or_create(&empty).set(s.reference_slot);
        self.sync_lag_slots.get_or_create(&empty).set(s.sync_lag_slots);
        self.block_height.get_or_create(&empty).set(s.block_height);
        self.epoch.get_or_create(&empty).set(s.epoch);
        self.slot_index.get_or_create(&empty).set(s.slot_index);
        self.slots_in_epoch.get_or_create(&empty).set(s.slots_in_epoch);
        self.max_shred_insert_slot.get_or_create(&empty).set(s.max_shred_insert_slot);
        self.shred_gap_slots.get_or_create(&empty).set(s.shred_gap_slots);
        self.highest_snapshot_full.get_or_create(&empty).set(s.highest_snapshot_full);
        self.highest_snapshot_incremental.get_or_create(&empty).set(s.highest_snapshot_incremental);
        self.snapshot_lag_slots.get_or_create(&empty).set(s.snapshot_lag_slots);
        self.validator_delinquent.get_or_create(&node).set(s.delinquent);
        self.validator_activated_stake_sol.get_or_create(&node).set(s.activated_stake_sol);
        self.validator_commission_pct.get_or_create(&node).set(s.commission_pct);
        self.validator_last_vote.get_or_create(&node).set(s.last_vote);
        self.validator_last_vote_lag_slots.get_or_create(&node).set(s.last_vote_lag_slots);
        self.validator_root_slot.get_or_create(&node).set(s.root_slot);
        self.validator_root_lag_slots.get_or_create(&node).set(s.root_lag_slots);
        self.cluster_confirmed_blocks_current_epoch.get_or_create(&empty).set(s.cluster_confirmed_blocks_current_epoch);
        self.identity_balance_sol.get_or_create(&node).set(s.identity_balance_sol);
        self.vote_balance_sol.get_or_create(&vote).set(s.vote_balance_sol);
        self.version_info.get_or_create(&ver).set(1.0);
        self.scrape_success.get_or_create(&empty).set(s.scrape_success);
        self.scrape_errors_total.get_or_create(&empty).set(s.scrape_errors_total);
        self.scrape_duration_ms.get_or_create(&empty).set(s.scrape_duration_ms);
        self.scrape_timestamp_unix.get_or_create(&empty).set(s.scrape_timestamp_unix);
        self.last_success_timestamp_unix.get_or_create(&empty).set(s.last_success_timestamp_unix);
    }
}

fn reg<M: Clone>(registry: &mut Registry, name: &str, help: &str, metric: M)
where
    M: prometheus_client::registry::Metric,
{
    registry.register(name, help, metric);
}

#[derive(Debug, Deserialize)]
struct RpcResponse<T> {
    result: T,
}

#[derive(Debug, Deserialize)]
struct HealthResponse {
    result: Option<String>,
    error: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct VersionResult {
    #[serde(rename = "solana-core")]
    solana_core: String,
}

#[derive(Debug, Deserialize)]
struct EpochInfo {
    #[serde(rename = "blockHeight")]
    block_height: u64,
    epoch: u64,
    #[serde(rename = "slotIndex")]
    slot_index: u64,
    #[serde(rename = "slotsInEpoch")]
    slots_in_epoch: u64,
}

#[derive(Debug, Deserialize)]
struct VoteAccounts {
    current: Vec<VoteAccount>,
    delinquent: Vec<VoteAccount>,
}

#[derive(Debug, Clone, Deserialize)]
struct VoteAccount {
    #[serde(rename = "activatedStake")]
    activated_stake: u64,
    commission: u64,
    #[serde(rename = "epochCredits")]
    epoch_credits: Vec<(u64, u64, u64)>,
    #[serde(rename = "lastVote")]
    last_vote: u64,
    #[serde(rename = "nodePubkey")]
    #[allow(dead_code)]
    node_pubkey: String,
    #[serde(rename = "rootSlot")]
    root_slot: u64,
    #[serde(rename = "votePubkey")]
    vote_pubkey: String,
}

#[derive(Debug, Deserialize)]
struct BlockProductionEnvelope {
    value: BlockProductionValue,
}

#[derive(Debug, Deserialize)]
struct BlockProductionValue {
    #[serde(rename = "byIdentity")]
    by_identity: HashMap<String, (u64, u64)>,
}

#[derive(Debug, Deserialize)]
struct SnapshotSlots {
    full: u64,
    incremental: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let config_path = std::env::args().nth(1).unwrap_or_else(|| "config/config.toml".to_string());
    let cfg = load_config(&config_path)?;
    tracing_subscriber::fmt()
        .with_env_filter(cfg.exporter.log_level.clone())
        .init();

    ensure_parent_dir(&cfg.exporter.state_path)?;

    let client = Client::builder()
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(cfg.rpc.timeout_secs))
        .build()
        .context("build reqwest client")?;

    let mut registry = Registry::default();
    let _metrics = Metrics::new(&mut registry);
    let app_state = AppState { registry: Arc::new(RwLock::new(registry)) };
    let reg_handle = app_state.registry.clone();
    let cfg_bg = cfg.clone();

    tokio::spawn(async move {
        let mut state = load_state(&cfg_bg.exporter.state_path).unwrap_or_default();
        let mut errors_total = 0.0;
        let mut last_good = Snapshot {
            uptime_target: cfg_bg.slo.uptime_target_pct,
            voting_effectiveness_target: cfg_bg.slo.voting_effectiveness_target_pct,
            skip_rate_target: cfg_bg.slo.skip_rate_target_pct,
            ..Snapshot::default()
        };

        loop {
            let started = std::time::Instant::now();
            let mut published = match collect_once(&client, &cfg_bg, &mut state).await {
                Ok(mut snap) => {
                    snap.scrape_success = 1.0;
                    snap.last_success_timestamp_unix = Utc::now().timestamp() as f64;
                    last_good = snap.clone();
                    snap
                }
                Err(e) => {
                    errors_total += 1.0;
                    warn!("collection failed: {:#}", e);
                    let mut snap = last_good.clone();
                    snap.scrape_success = 0.0;
                    snap.scrape_errors_total = errors_total;
                    snap
                }
            };

            published.scrape_duration_ms = started.elapsed().as_millis() as f64;
            published.scrape_timestamp_unix = Utc::now().timestamp() as f64;
            published.scrape_errors_total = errors_total;

            persist_state(&cfg_bg.exporter.state_path, &state).ok();

            let mut fresh = Registry::default();
            let fresh_metrics = Metrics::new(&mut fresh);
            fresh_metrics.update(&cfg_bg, &published);
            let mut reg = reg_handle.write().await;
            *reg = fresh;

            info!(
                mode = ?cfg_bg.mode,
                scrape_success = published.scrape_success,
                health = published.rpc_health,
                uptime = published.uptime_ratio,
                voting = published.voting_effectiveness_pct,
                skip = published.skip_rate_pct,
                sync_lag = published.sync_lag_slots,
                "collection finished"
            );

            sleep(TokioDuration::from_secs(cfg_bg.exporter.poll_interval_secs)).await;
        }
    });

    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(app_state);

    let addr: SocketAddr = cfg.exporter.listen_addr.parse().context("parse listen_addr")?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("listening on {}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    let mut out = String::new();
    let reg = state.registry.read().await;
    match encode(&mut out, &reg) {
        Ok(()) => (axum::http::StatusCode::OK, out),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("encode error: {e}"),
        ),
    }
}

async fn collect_once(client: &Client, cfg: &Config, uptime_state: &mut PersistedState) -> Result<Snapshot> {
    let version: VersionResult = rpc(client, &cfg.rpc.target_url, "getVersion", json!([])).await?;
    let health_ok = rpc_health(client, &cfg.rpc.target_url).await?;
    let epoch_info: EpochInfo =
        rpc(client, &cfg.rpc.target_url, "getEpochInfo", json!([{"commitment": cfg.rpc.commitment}])).await?;
    let local_slot: u64 = rpc(client, &cfg.rpc.target_url, "getSlot", json!([{"commitment": cfg.rpc.commitment}])).await?;
    let reference_slot: u64 = rpc(client, &cfg.rpc.reference_url, "getSlot", json!([{"commitment": cfg.rpc.commitment}])).await?;
    let max_shred_insert_slot: u64 =
        rpc(client, &cfg.rpc.target_url, "getMaxShredInsertSlot", json!([])).await.unwrap_or(local_slot);
    let snapshot_slots: SnapshotSlots = rpc(client, &cfg.rpc.target_url, "getHighestSnapshotSlot", json!([]))
        .await
        .unwrap_or(SnapshotSlots { full: 0, incremental: 0 });

    let identity_balance_lamports: u64 = if cfg.features.enable_balance_metrics {
        get_balance(client, &cfg.rpc.target_url, &cfg.identity.node_pubkey, &cfg.rpc.commitment).await.unwrap_or(0)
    } else {
        0
    };

    let mut snapshot = Snapshot {
        solana_version: version.solana_core,
        rpc_health: bool01(health_ok),
        absolute_slot: local_slot as f64,
        reference_slot: reference_slot as f64,
        sync_lag_slots: reference_slot.saturating_sub(local_slot) as f64,
        block_height: epoch_info.block_height as f64,
        epoch: epoch_info.epoch as f64,
        slot_index: epoch_info.slot_index as f64,
        slots_in_epoch: epoch_info.slots_in_epoch as f64,
        max_shred_insert_slot: max_shred_insert_slot as f64,
        shred_gap_slots: max_shred_insert_slot.saturating_sub(local_slot) as f64,
        highest_snapshot_full: snapshot_slots.full as f64,
        highest_snapshot_incremental: snapshot_slots.incremental as f64,
        snapshot_lag_slots: local_slot.saturating_sub(snapshot_slots.incremental) as f64,
        identity_balance_sol: lamports_to_sol(identity_balance_lamports),
        uptime_target: cfg.slo.uptime_target_pct,
        voting_effectiveness_target: cfg.slo.voting_effectiveness_target_pct,
        skip_rate_target: cfg.slo.skip_rate_target_pct,
        ..Snapshot::default()
    };

    let mut observed_up = !cfg.uptime.require_health_ok || health_ok;
    if cfg.features.enable_sync_metrics && snapshot.sync_lag_slots as u64 > cfg.uptime.max_sync_lag_slots {
        observed_up = false;
    }

    if matches!(cfg.mode, Mode::Validator) {
        let vote_pubkey = cfg.identity.vote_pubkey.clone().context("vote_pubkey required in validator mode")?;
        let vote_accounts: VoteAccounts = rpc(
            client,
            &cfg.rpc.target_url,
            "getVoteAccounts",
            json!([{ "commitment": cfg.rpc.commitment, "votePubkey": vote_pubkey }]),
        )
        .await?;

        let current = vote_accounts.current.first().cloned();
        let delinquent = vote_accounts.delinquent.first().cloned();
        let vote = current.clone().or(delinquent.clone()).context("vote account not found")?;
        let is_delinquent = delinquent.is_some();

        let block_production: BlockProductionEnvelope = if cfg.features.enable_skip_rate {
            rpc(client, &cfg.rpc.target_url, "getBlockProduction", json!([{ "commitment": cfg.rpc.commitment }])).await?
        } else {
            BlockProductionEnvelope { value: BlockProductionValue { by_identity: HashMap::new() } }
        };

        let (leader_slots, produced_blocks) = block_production
            .value
            .by_identity
            .get(&cfg.identity.node_pubkey)
            .copied()
            .unwrap_or((0, 0));

        let cluster_confirmed_blocks: u64 = block_production.value.by_identity.values().map(|v| v.1).sum();
        let skipped_slots = leader_slots.saturating_sub(produced_blocks);
        let skip_rate_pct = if leader_slots > 0 { (skipped_slots as f64 / leader_slots as f64) * 100.0 } else { 0.0 };
        let credits_earned = vote
            .epoch_credits
            .iter()
            .find(|(epoch, _, _)| *epoch == epoch_info.epoch)
            .map(|(_, credits, prev)| credits.saturating_sub(*prev))
            .unwrap_or(0);
        let voting_effectiveness_pct = if cluster_confirmed_blocks > 0 {
            (credits_earned as f64 / (cluster_confirmed_blocks as f64 * 16.0)) * 100.0
        } else {
            0.0
        };

        let vote_balance_sol = if cfg.features.enable_balance_metrics {
            lamports_to_sol(get_balance(client, &cfg.rpc.target_url, &vote.vote_pubkey, &cfg.rpc.commitment).await.unwrap_or(0))
        } else {
            0.0
        };

        snapshot.validator_present = 1.0;
        snapshot.delinquent = bool01(is_delinquent);
        snapshot.activated_stake_sol = lamports_to_sol(vote.activated_stake);
        snapshot.commission_pct = vote.commission as f64;
        snapshot.credits_earned_current_epoch = credits_earned as f64;
        snapshot.last_vote = vote.last_vote as f64;
        snapshot.root_slot = vote.root_slot as f64;
        snapshot.last_vote_lag_slots = local_slot.saturating_sub(vote.last_vote) as f64;
        snapshot.root_lag_slots = local_slot.saturating_sub(vote.root_slot) as f64;
        snapshot.leader_slots_current_epoch = leader_slots as f64;
        snapshot.produced_blocks_current_epoch = produced_blocks as f64;
        snapshot.skipped_slots_current_epoch = skipped_slots as f64;
        snapshot.skip_rate_pct = skip_rate_pct;
        snapshot.cluster_confirmed_blocks_current_epoch = cluster_confirmed_blocks as f64;
        snapshot.voting_effectiveness_pct = voting_effectiveness_pct;
        snapshot.vote_balance_sol = vote_balance_sol;
        snapshot.voting_effectiveness_slo_met = bool01(voting_effectiveness_pct >= cfg.slo.voting_effectiveness_target_pct);
        snapshot.skip_rate_slo_met = bool01(skip_rate_pct <= cfg.slo.skip_rate_target_pct);

        if cfg.uptime.require_not_delinquent && is_delinquent {
            observed_up = false;
        }
        if snapshot.last_vote_lag_slots as u64 > cfg.uptime.max_last_vote_lag_slots {
            observed_up = false;
        }
        if snapshot.root_lag_slots as u64 > cfg.uptime.max_root_lag_slots {
            observed_up = false;
        }
    }

    push_uptime_sample(uptime_state, observed_up, cfg.uptime.window_hours);
    snapshot.uptime_ratio = calc_uptime_ratio(uptime_state) * 100.0;
    snapshot.uptime_slo_met = bool01(snapshot.uptime_ratio >= cfg.slo.uptime_target_pct);

    Ok(snapshot)
}

async fn get_balance(client: &Client, rpc_url: &str, address: &str, commitment: &str) -> Result<u64> {
    #[derive(Deserialize)]
    struct BalanceValue { value: u64 }
    let resp: BalanceValue = rpc(client, rpc_url, "getBalance", json!([address, {"commitment": commitment}])).await?;
    Ok(resp.value)
}

fn load_config(path: &str) -> Result<Config> {
    let raw = fs::read_to_string(path).with_context(|| format!("read config {path}"))?;
    let cfg: Config = toml::from_str(&raw).with_context(|| format!("parse config {path}"))?;
    Ok(cfg)
}

fn ensure_parent_dir(path: &str) -> Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent).ok();
    }
    Ok(())
}

fn load_state(path: &str) -> Result<PersistedState> {
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn persist_state(path: &str, state: &PersistedState) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(state)?)?;
    Ok(())
}

fn push_uptime_sample(state: &mut PersistedState, up: bool, window_hours: i64) {
    let now = Utc::now();
    state.samples.push(UptimeSample { ts: now, up });
    let cutoff = now - Duration::hours(window_hours);
    state.samples.retain(|s| s.ts >= cutoff);
}

fn calc_uptime_ratio(state: &PersistedState) -> f64 {
    if state.samples.is_empty() {
        return 0.0;
    }
    state.samples.iter().filter(|s| s.up).count() as f64 / state.samples.len() as f64
}

fn lamports_to_sol(v: u64) -> f64 {
    v as f64 / 1_000_000_000.0
}

fn bool01(v: bool) -> f64 {
    if v { 1.0 } else { 0.0 }
}

async fn rpc<T: DeserializeOwned>(client: &Client, rpc_url: &str, method: &str, params: Value) -> Result<T> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params
    });
    let resp = client
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("rpc send failed for {method}"))?
        .error_for_status()
        .with_context(|| format!("rpc http error for {method}"))?;
    let envelope: RpcResponse<T> = resp.json().await.with_context(|| format!("rpc decode failed for {method}"))?;
    Ok(envelope.result)
}

async fn rpc_health(client: &Client, rpc_url: &str) -> Result<bool> {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "getHealth" });
    let resp = client.post(rpc_url).json(&body).send().await?.error_for_status()?;
    let health: HealthResponse = resp.json().await?;
    Ok(health.result.unwrap_or_default() == "ok" && health.error.is_none())
}
