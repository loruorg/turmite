use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use rust_decimal::Decimal;

use crate::error::Result;

/// A per-payment deposit address derived from a wallet account.
#[derive(Clone, Debug)]
pub struct DerivedAddress {
    pub address: String,
    pub payin_extra_id: Option<String>,
    pub derivation_index: u64,
}

/// A raw on-chain detection of funds at a watched address.
#[derive(Clone, Debug)]
pub struct Observation {
    /// Chain-native idempotency key: `txid:vout` / `signature` / `txid`.
    pub chain_tx_key: String,
    pub txid: String,
    pub vout: Option<i32>,
    /// Amount credited to the watched address, in asset units.
    pub amount: Decimal,
    pub confirmations: i32,
    pub block_height: Option<i64>,
}

/// Move the funds at a deposit address to the project's settlement address.
#[derive(Clone, Debug)]
pub struct SweepRequest {
    pub from_address: String,
    pub to_address: String,
    pub to_extra_id: Option<String>,
    /// Amount to move, in asset units.
    pub amount: Decimal,
    pub derivation_index: u64,
}

#[derive(Clone, Debug)]
pub struct SweepResult {
    pub txid: String,
    pub network_fee: Decimal,
}

/// A broadcast sweep to check on chain.
#[derive(Clone, Debug)]
pub struct SweepCheck {
    pub txid: String,
    pub from_address: String,
    pub to_address: String,
    /// When the sweep was recorded, just after the broadcast.
    pub broadcast_at: chrono::DateTime<chrono::Utc>,
}

impl SweepCheck {
    /// True when the chain can no longer take the transaction: each sweep is valid for a few
    /// minutes only (Tron expiration, Solana blockhash, TON `valid_until`).
    pub fn expired(&self) -> bool {
        chrono::Utc::now() - self.broadcast_at > chrono::Duration::minutes(SWEEP_EXPIRY_MINUTES)
    }
}

/// A sweep that the chain does not know this long after its broadcast never lands.
pub const SWEEP_EXPIRY_MINUTES: i64 = 10;

/// What the chain says about a broadcast sweep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SweepState {
    /// Not final yet.
    Pending,
    /// Final on chain: the funds moved.
    Confirmed,
    /// The funds did not move and the transaction can no longer land. The reason is for the operator.
    Failed(String),
}

/// The asset being moved on a chain: native coin (no contract) or a token.
/// Threaded into observe/sweep so one driver per chain handles both.
#[derive(Clone, Debug)]
pub struct AssetContext {
    pub decimals: u32,
    /// None = native coin; Some = token contract / mint / TRC20 address.
    pub contract_address: Option<String>,
}

impl AssetContext {
    pub fn is_native(&self) -> bool {
        self.contract_address.is_none()
    }
    pub fn native(decimals: u32) -> Self {
        Self { decimals, contract_address: None }
    }
    pub fn from_asset_network(an: &crate::domain::AssetNetwork) -> Self {
        Self { decimals: an.decimals as u32, contract_address: an.contract_address.clone() }
    }
}

/// A project's settlement setup on one network, as drivers need it.
#[derive(Clone, Debug, Default)]
pub struct WalletContext {
    /// 0 for addresses made before a project takes them (the shared pool).
    pub project_id: i64,
    /// Settlement or cold wallet address.
    pub address: Option<String>,
    pub address_extra_id: Option<String>,
    /// Bitcoin account xpub (watch-only).
    pub xpub: Option<String>,
    /// Monero private view key.
    pub view_key: Option<zeroize::Zeroizing<String>>,
    pub restore_height: Option<i64>,
    /// When the project wallet was saved. No deposit to it can be older.
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    /// EVM deposit factory of the project.
    pub factory_address: Option<String>,
    /// The project's hot key for this family (worker only, at sweep time): 32-byte secret.
    pub hot_secret: Option<zeroize::Zeroizing<Vec<u8>>>,
    pub hot_address: Option<String>,
}

/// Where derivation indexes come from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndexScope {
    /// One counter per network for the whole install (keys from the deposit seed).
    Network,
    /// One counter per project wallet (xpub, view wallet).
    Wallet,
    /// One counter for the whole chain family (EVM deposit salts: one salt gives the same address on every chain).
    Family,
}

/// A chain connector. One implementation per chain family.
#[async_trait]
pub trait PaymentDriver: Send + Sync {
    /// The `network.chain` value this driver serves (e.g. "bitcoin").
    fn chain(&self) -> &str;
    fn index_scope(&self) -> IndexScope {
        IndexScope::Network
    }
    /// False when this process cannot derive deposit addresses (a seed-derived chain
    /// in the API process). Then addresses come from the pool the worker fills.
    fn can_derive(&self) -> bool {
        true
    }
    async fn validate_address(&self, address: &str) -> Result<bool>;
    /// Check a project's wallet setup when it is saved and prepare what the chain needs
    /// (for example a watch-only wallet). Returns lines to show the operator.
    async fn prepare_wallet(&self, _w: &WalletContext) -> Result<Vec<String>> {
        Ok(vec![])
    }
    async fn derive_deposit(&self, w: &WalletContext, derivation_index: u64) -> Result<DerivedAddress>;
    /// Incoming funds (native coin or `asset`'s token) at a watched address.
    async fn observe(
        &self,
        w: &WalletContext,
        address: &str,
        payin_extra_id: Option<&str>,
        asset: &AssetContext,
    ) -> Result<Vec<Observation>>;
    async fn sweep(&self, w: &WalletContext, req: &SweepRequest, asset: &AssetContext) -> Result<SweepResult>;
    /// State of a sweep that `sweep` broadcast. Chains without sweeps never call it.
    async fn sweep_state(&self, _sweep: &SweepCheck, _asset: &AssetContext) -> Result<SweepState> {
        Ok(SweepState::Confirmed)
    }
    /// Height of the chain tip as the node sees it (health checks, "last block").
    async fn tip_height(&self) -> Result<i64> {
        Err(crate::error::Error::Driver("tip height is not available for this chain".into()))
    }
    /// False when deposits land in the merchant's own wallet (watch-only chains).
    fn needs_sweep(&self) -> bool {
        true
    }
    /// True for the in-memory mock driver (the demo can fake deposits).
    fn is_mock(&self) -> bool {
        false
    }
    fn as_mock(&self) -> Option<&MockDriver> {
        None
    }
    /// The concrete driver, for chain-specific work outside this trait (the EVM sweeper).
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        None
    }
}

/// Selects a driver by chain name.
#[derive(Clone, Default)]
pub struct DriverRegistry {
    drivers: HashMap<String, Arc<dyn PaymentDriver>>,
}

impl DriverRegistry {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn insert(&mut self, driver: Arc<dyn PaymentDriver>) {
        self.drivers.insert(driver.chain().to_string(), driver);
    }
    pub fn get(&self, chain: &str) -> Option<Arc<dyn PaymentDriver>> {
        self.drivers.get(chain).cloned()
    }
    pub fn chains(&self) -> impl Iterator<Item = &str> {
        self.drivers.keys().map(String::as_str)
    }
    /// A registry with a mock driver for every supported chain.
    pub fn all_mock() -> Self {
        let mut reg = Self::new();
        for chain in ["bitcoin", "monero", "ethereum", "base", "polygon", "bsc", "tron", "solana", "ton"] {
            reg.insert(Arc::new(MockDriver::new(chain)));
        }
        reg
    }
}

/// Deterministic in-memory driver for tests + dev. Detection is driven by an
/// injected observation map (the admin mock-settle endpoint / tests fill it).
#[derive(Clone)]
pub struct MockDriver {
    chain: String,
    observations: Arc<tokio::sync::Mutex<HashMap<String, Vec<Observation>>>>,
    /// Sweep states by txid. A sweep that is not here is confirmed.
    sweep_states: Arc<std::sync::Mutex<HashMap<String, SweepState>>>,
    /// False: act as an API process without the deposit seed (addresses come from the pool).
    derive: bool,
    /// When set, `sweep` fails with this message.
    sweep_error: Arc<std::sync::Mutex<Option<String>>>,
}

impl MockDriver {
    pub fn new(chain: &str) -> Self {
        Self {
            chain: chain.to_string(),
            observations: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            sweep_states: Arc::new(std::sync::Mutex::new(HashMap::new())),
            derive: true,
            sweep_error: Arc::new(std::sync::Mutex::new(None)),
        }
    }
    /// A mock that cannot derive deposit addresses, as a seed-derived chain in the API process.
    pub fn without_derive(chain: &str) -> Self {
        Self { derive: false, ..Self::new(chain) }
    }
    /// Test hook: stage an observation that a later `observe(address)` returns.
    pub async fn inject(&self, address: &str, obs: Observation) {
        self.observations.lock().await.entry(address.to_string()).or_default().push(obs);
    }
    /// Test hook: what `sweep_state` answers for a sweep.
    pub fn set_sweep_state(&self, txid: &str, state: SweepState) {
        self.sweep_states.lock().unwrap_or_else(|e| e.into_inner()).insert(txid.to_string(), state);
    }
    /// Test hook: make `sweep` fail with `error`, or work again with `None`.
    pub fn fail_sweeps(&self, error: Option<&str>) {
        *self.sweep_error.lock().unwrap_or_else(|e| e.into_inner()) = error.map(str::to_string);
    }
}

#[async_trait]
impl PaymentDriver for MockDriver {
    fn chain(&self) -> &str {
        &self.chain
    }
    fn is_mock(&self) -> bool {
        true
    }
    fn as_mock(&self) -> Option<&MockDriver> {
        Some(self)
    }
    fn can_derive(&self) -> bool {
        self.derive
    }
    /// Bitcoin and Monero are watch-only: deposits land in the merchant's wallet.
    fn needs_sweep(&self) -> bool {
        !matches!(self.chain.as_str(), "bitcoin" | "monero")
    }
    async fn validate_address(&self, address: &str) -> Result<bool> {
        Ok(!address.trim().is_empty())
    }
    async fn derive_deposit(&self, _w: &WalletContext, derivation_index: u64) -> Result<DerivedAddress> {
        Ok(DerivedAddress { address: format!("mock-{}-{derivation_index:08}", self.chain), payin_extra_id: None, derivation_index })
    }
    async fn observe(
        &self,
        _w: &WalletContext,
        address: &str,
        _payin_extra_id: Option<&str>,
        _asset: &AssetContext,
    ) -> Result<Vec<Observation>> {
        Ok(self.observations.lock().await.get(address).cloned().unwrap_or_default())
    }
    async fn sweep(&self, _w: &WalletContext, req: &SweepRequest, _asset: &AssetContext) -> Result<SweepResult> {
        if let Some(e) = self.sweep_error.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Err(crate::error::Error::Driver(e));
        }
        // Unique per broadcast (real drivers return a unique on-chain txid;
        // a payment may be swept multiple times for top-ups/overpayments).
        Ok(SweepResult {
            txid: format!("mocktx-{}-{}-{}", self.chain, req.derivation_index, uuid::Uuid::new_v4().simple()),
            network_fee: Decimal::ZERO,
        })
    }
    async fn sweep_state(&self, sweep: &SweepCheck, _asset: &AssetContext) -> Result<SweepState> {
        Ok(self.sweep_states.lock().unwrap_or_else(|e| e.into_inner()).get(&sweep.txid).cloned().unwrap_or(SweepState::Confirmed))
    }
}

pub fn as_mock(d: &Arc<dyn PaymentDriver>) -> Option<&MockDriver> {
    d.as_mock()
}
