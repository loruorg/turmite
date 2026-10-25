//! EVM driver (Ethereum, Base, Polygon, BNB Smart Chain), one instance per network.
//!
//! Each payment gets a deposit address of the project's `DepositFactory` (see `evm_factory`). The funds at
//! that address can only go to the cold wallet fixed in the factory. The worker's EVM sweeper flushes them
//! in batches (`worker::evm_sweeper`); the project's hot key only pays gas.
//!
//! Detection keeps a small state per deposit address and asset (`evm_scans`):
//! - tokens: one arrival per `Transfer` log to the deposit address;
//! - native coin: there are no logs, so the received total is the balance plus the `Flushed` amounts of
//!   the factory, and each increase is dated to the first block that shows it.
//!
//! Arrivals in the last `depth` blocks are read again on every pass, so a reorg removes them.

use std::{
    str::FromStr,
    sync::Mutex,
    time::{Duration, Instant},
};

use alloy::{
    consensus::Transaction as _,
    eips::{BlockNumberOrTag, eip2718::Encodable2718},
    network::{Ethereum, EthereumWallet, NetworkTransactionBuilder, TransactionBuilder},
    primitives::{Address, B256, Bytes, U256},
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::{Filter, Log, TransactionReceipt, TransactionRequest},
    signers::local::PrivateKeySigner,
    sol_types::SolEvent,
};
use async_trait::async_trait;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{
    chains::{
        driver::{AssetContext, DerivedAddress, IndexScope, Observation, PaymentDriver, SweepRequest, SweepResult, WalletContext},
        evm_factory::{self, DepositFactory, NATIVE},
    },
    db::PgStore,
    domain::Network,
    error::{Error, Result},
};

alloy::sol! {
    #[sol(rpc)]
    #[allow(missing_docs)]
    interface IERC20 {
        event Transfer(address indexed from, address indexed to, uint256 value);
        function balanceOf(address owner) external view returns (uint256);
    }
}

const RPC_TIMEOUT: Duration = Duration::from_secs(20);
/// Recent blocks whose state pruned nodes keep (geth and bor keep 128).
const STATE_SPAN: u64 = 120;
/// Largest block range of one `eth_getLogs` call. It gets smaller when a node refuses a range.
const LOG_SPAN: u64 = 2_000;
/// Highest `maxFeePerGas` the hot wallet pays (1000 gwei). Above it, sends wait.
pub const MAX_FEE_PER_GAS: u128 = 1_000_000_000_000;
const TIP_TTL: Duration = Duration::from_secs(1);

fn rpc<E: std::fmt::Display>(what: &'static str) -> impl FnOnce(E) -> Error {
    move |e| Error::Driver(format!("{what}: {e}"))
}

/// Parse an address with the EIP-55 rules: all lower case or all upper case has no checksum; mixed case
/// must match the checksum. The zero address is refused.
pub fn parse_address(s: &str) -> Option<Address> {
    let hex = s.strip_prefix("0x")?;
    if hex.len() != 40 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let address = Address::from_str(s).ok()?;
    let mixed = hex.bytes().any(|b| b.is_ascii_lowercase()) && hex.bytes().any(|b| b.is_ascii_uppercase());
    if (mixed && address.to_checksum(None) != s) || address.is_zero() {
        return None;
    }
    Some(address)
}

/// Lower-case `0x` form, used as a key in the database.
pub fn key(a: &Address) -> String {
    format!("{a:#x}")
}

/// Base units to asset units.
pub fn to_decimal(raw: U256, decimals: u32) -> Decimal {
    let scale = decimals.min(28);
    if let Ok(v) = i128::try_from(raw)
        && let Ok(d) = Decimal::try_from_i128_with_scale(v, scale)
    {
        return d;
    }
    // Above 2^96 base units: lose precision, keep the size.
    Decimal::from_str(&(raw / U256::from(10u64).pow(U256::from(scale))).to_string()).unwrap_or(Decimal::MAX)
}

/// Asset units to base units (rounded down).
pub fn to_raw(amount: Decimal, decimals: u32) -> U256 {
    let scaled = (amount * Decimal::from(10u64.pow(decimals.min(18)))).trunc();
    U256::from_str(&scaled.to_string()).unwrap_or(U256::ZERO)
}

/// One incoming transfer, or one increase of the native balance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Arrival {
    key: String,
    txid: String,
    block: u64,
    amount: U256,
    #[serde(default)]
    log_index: Option<u64>,
    #[serde(default, rename = "final")]
    fin: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct ScanState {
    final_to: u64,
    final_flushed: U256,
    arrivals: Vec<Arrival>,
}

impl ScanState {
    fn new(final_to: u64) -> Self {
        Self { final_to, final_flushed: U256::ZERO, arrivals: vec![] }
    }

    fn final_received(&self) -> U256 {
        self.arrivals.iter().filter(|a| a.fin).fold(U256::ZERO, |s, a| s.saturating_add(a.amount))
    }

    /// Replace the arrivals after `final_to` with `fresh` (all arrivals in `(final_to, tip]`) and move
    /// `final_to` up to `tip - depth`. Returns the arrivals that were reported before and are gone now.
    fn merge(&mut self, fresh: Vec<Arrival>, tip: u64, depth: u64) -> Vec<Arrival> {
        let (window, mut kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.arrivals).into_iter().partition(|a| !a.fin);
        let final_to = self.final_to.max(tip.saturating_sub(depth));
        let gone = window.into_iter().filter(|a| !fresh.iter().any(|f| f.key == a.key)).collect();
        kept.extend(fresh.into_iter().map(|a| Arrival { fin: a.block <= final_to, ..a }));
        kept.sort_by(|a, b| (a.block, &a.key).cmp(&(b.block, &b.key)));
        self.arrivals = kept;
        self.final_to = final_to;
        gone
    }
}

fn observation(a: &Arrival, tip: u64, decimals: u32) -> Observation {
    let confirmations = if tip >= a.block { (tip - a.block + 1).min(i32::MAX as u64) as i32 } else { 0 };
    Observation {
        chain_tx_key: a.key.clone(),
        txid: a.txid.clone(),
        vout: a.log_index.map(|i| i as i32),
        amount: to_decimal(a.amount, decimals),
        confirmations,
        block_height: Some(a.block as i64),
    }
}

/// An arrival that a reorg removed: it must not count any more.
fn removed(a: &Arrival) -> Observation {
    Observation {
        chain_tx_key: a.key.clone(),
        txid: a.txid.clone(),
        vout: a.log_index.map(|i| i as i32),
        amount: Decimal::ZERO,
        confirmations: 0,
        block_height: None,
    }
}

pub struct EvmDriver {
    chain: String,
    code: String,
    network_id: i64,
    chain_id: u64,
    /// Arrivals in the last `depth` blocks are read again on each pass.
    depth: u64,
    provider: DynProvider,
    store: PgStore,
    tip: Mutex<Option<(Instant, u64)>>,
}

impl EvmDriver {
    /// Connect to a node of `network`. Refuses a node on another chain id.
    pub async fn connect(network: &Network, url: &str, store: PgStore) -> Result<Self> {
        let expected = network.chain_id.ok_or_else(|| Error::Driver(format!("{} has no chain id", network.code)))?;
        let parsed = url::Url::parse(url).map_err(rpc("node url"))?;
        let client = reqwest::Client::builder().timeout(RPC_TIMEOUT).build().map_err(rpc("http client"))?;
        let provider = ProviderBuilder::new().disable_recommended_fillers().connect_reqwest(client, parsed).erased();
        let got = provider.get_chain_id().await.map_err(rpc("chain id"))?;
        if i64::try_from(got).ok() != Some(expected) {
            return Err(Error::Driver(format!("the node is on chain id {got}, but {} needs chain id {expected}", network.name)));
        }
        let confirmations = network.confirmations_required.max(1) as u64;
        Ok(Self {
            chain: network.chain.clone(),
            code: network.code.clone(),
            network_id: network.network_id,
            chain_id: got,
            depth: (confirmations * 2).clamp(12, STATE_SPAN),
            provider,
            store,
            tip: Mutex::new(None),
        })
    }

    pub fn network_id(&self) -> i64 {
        self.network_id
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    /// Latest block number, cached for a second.
    pub async fn tip(&self) -> Result<u64> {
        if let Some((at, n)) = *self.tip.lock().unwrap_or_else(|e| e.into_inner())
            && at.elapsed() < TIP_TTL
        {
            return Ok(n);
        }
        let n = self.provider.get_block_number().await.map_err(rpc("block number"))?;
        *self.tip.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), n));
        Ok(n)
    }

    pub async fn has_code(&self, a: Address) -> Result<bool> {
        Ok(!self.provider.get_code_at(a).await.map_err(rpc("code"))?.is_empty())
    }

    pub async fn balance(&self, a: Address) -> Result<U256> {
        self.provider.get_balance(a).await.map_err(rpc("balance"))
    }

    pub async fn token_balance(&self, token: Address, owner: Address) -> Result<U256> {
        IERC20::new(token, &self.provider).balanceOf(owner).call().await.map_err(rpc("balanceOf"))
    }

    /// EIP-1559 fees `(max fee, priority fee)`: twice the base fee plus the node's priority fee.
    /// Fails above `MAX_FEE_PER_GAS`.
    pub async fn fees(&self) -> Result<(u128, u128)> {
        let block = self
            .provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await
            .map_err(rpc("latest block"))?
            .ok_or_else(|| Error::Driver("the node has no latest block".into()))?;
        let base = block.header.base_fee_per_gas.unwrap_or(0) as u128;
        let priority = self.provider.get_max_priority_fee_per_gas().await.map_err(rpc("priority fee"))?;
        let max_fee = base.saturating_mul(2).saturating_add(priority);
        if max_fee > MAX_FEE_PER_GAS {
            return Err(Error::Driver(format!("{}: gas price {max_fee} wei is above the cap", self.code)));
        }
        Ok((max_fee, priority))
    }

    pub async fn estimate_gas(&self, from: Address, to: Address, data: Bytes) -> Result<u64> {
        let tx = TransactionRequest::default().with_from(from).with_to(to).with_input(data);
        self.provider.estimate_gas(tx).await.map_err(rpc("estimate gas"))
    }

    pub async fn pending_nonce(&self, a: Address) -> Result<u64> {
        self.provider.get_transaction_count(a).pending().await.map_err(rpc("nonce"))
    }

    /// Sign an EIP-1559 call. Returns the hash and the raw transaction.
    pub async fn sign(
        &self,
        signer: &PrivateKeySigner,
        nonce: u64,
        to: Address,
        data: Bytes,
        gas: u64,
        (max_fee, priority): (u128, u128),
    ) -> Result<(B256, Vec<u8>)> {
        let tx = TransactionRequest::default()
            .with_from(signer.address())
            .with_to(to)
            .with_input(data)
            .with_value(U256::ZERO)
            .with_nonce(nonce)
            .with_chain_id(self.chain_id)
            .with_gas_limit(gas)
            .with_max_fee_per_gas(max_fee)
            .with_max_priority_fee_per_gas(priority);
        let wallet = EthereumWallet::from(signer.clone());
        let envelope = <TransactionRequest as NetworkTransactionBuilder<Ethereum>>::build(tx, &wallet).await.map_err(rpc("sign"))?;
        Ok((*envelope.tx_hash(), envelope.encoded_2718()))
    }

    pub async fn broadcast(&self, raw: &[u8]) -> Result<()> {
        let _pending = self.provider.send_raw_transaction(raw).await.map_err(rpc("broadcast"))?;
        Ok(())
    }

    pub async fn receipt(&self, hash: B256) -> Result<Option<TransactionReceipt>> {
        self.provider.get_transaction_receipt(hash).await.map_err(rpc("receipt"))
    }

    /// Poll for a receipt until `timeout`.
    pub async fn wait_receipt(&self, hash: B256, timeout: Duration) -> Result<Option<TransactionReceipt>> {
        let start = Instant::now();
        loop {
            if let Some(r) = self.receipt(hash).await? {
                return Ok(Some(r));
            }
            if start.elapsed() >= timeout {
                return Ok(None);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// True when the node knows the transaction (pending or mined).
    pub async fn tx_known(&self, hash: B256) -> Result<bool> {
        Ok(self.provider.get_transaction_by_hash(hash).await.map_err(rpc("transaction"))?.is_some())
    }

    /// Logs in `[from, to]`, in ranges the node accepts.
    async fn logs(&self, filter: Filter, from: u64, to: u64) -> Result<Vec<Log>> {
        let mut out = Vec::new();
        let (mut start, mut span) = (from, LOG_SPAN);
        while start <= to {
            let end = to.min(start.saturating_add(span - 1));
            match self.provider.get_logs(&filter.clone().from_block(start).to_block(end)).await {
                Ok(mut logs) => {
                    out.append(&mut logs);
                    start = end + 1;
                }
                Err(_) if span > 10 => span /= 5,
                Err(e) => return Err(Error::Driver(format!("logs: {e}"))),
            }
        }
        out.retain(|l| !l.removed);
        Ok(out)
    }

    /// Token arrivals: `Transfer` logs to the deposit address in `[from, to]`.
    async fn token_arrivals(&self, token: Address, deposit: Address, from: u64, to: u64) -> Result<Vec<Arrival>> {
        let filter = Filter::new().address(token).event_signature(IERC20::Transfer::SIGNATURE_HASH).topic2(deposit.into_word());
        let logs = self.logs(filter, from, to).await?;
        Ok(logs
            .iter()
            .filter_map(|l| {
                let (block, hash, index) = (l.block_number?, l.transaction_hash?, l.log_index?);
                let amount = l.log_decode::<IERC20::Transfer>().ok()?.inner.data.value;
                let txid = format!("{hash:#x}");
                Some(Arrival { key: format!("{txid}:{index}"), txid, block, amount, log_index: Some(index), fin: false })
            })
            .collect())
    }

    /// Native coin that the factory flushed from the deposit address in `[from, to]`: (block, amount).
    async fn native_flushes(&self, factory: Address, deposit: Address, from: u64, to: u64) -> Result<Vec<(u64, U256)>> {
        let filter = Filter::new()
            .address(factory)
            .event_signature(DepositFactory::Flushed::SIGNATURE_HASH)
            .topic1(deposit.into_word())
            .topic2(NATIVE.into_word());
        let logs = self.logs(filter, from, to).await?;
        Ok(logs.iter().filter_map(|l| Some((l.block_number?, l.log_decode::<DepositFactory::Flushed>().ok()?.inner.data.amount))).collect())
    }

    /// Native coin received by the deposit address up to block `b`: balance plus what the factory flushed.
    async fn native_total(&self, deposit: Address, b: u64, flushed: &Flushed<'_>) -> Result<U256> {
        let balance = self.provider.get_balance(deposit).number(b).await.map_err(rpc("balance"))?;
        Ok(balance.saturating_add(flushed.at(b)))
    }

    /// The smallest block in `(lo, hi]` whose total is above `known`, and that total.
    /// `total_hi` (the total at `hi`) is above `known`. On a node error the answer moves later, never earlier.
    async fn first_above(&self, deposit: Address, lo: u64, hi: u64, known: U256, total_hi: U256, flushed: &Flushed<'_>) -> (u64, U256) {
        let (mut lo, mut hi, mut total_hi) = (lo, hi, total_hi);
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            match self.native_total(deposit, mid, flushed).await {
                Ok(t) if t > known => (hi, total_hi) = (mid, t),
                Ok(_) => lo = mid,
                Err(e) => {
                    tracing::debug!(network = %self.code, error = %e, "historic balance not available");
                    break;
                }
            }
        }
        (hi, total_hi)
    }

    /// Arrivals for an increase of `delta` at block `b`: one per direct transfer when they explain it,
    /// else one for the block.
    async fn native_arrivals(&self, deposit: Address, b: u64, delta: U256) -> Vec<Arrival> {
        if let Ok(transfers) = self.direct_transfers(deposit, b).await
            && !transfers.is_empty()
            && transfers.iter().fold(U256::ZERO, |s, (_, v)| s.saturating_add(*v)) == delta
        {
            return transfers
                .into_iter()
                .map(|(txid, amount)| Arrival { key: txid.clone(), txid, block: b, amount, log_index: None, fin: false })
                .collect();
        }
        let id = format!("{}@{b}", key(&deposit));
        vec![Arrival { key: id.clone(), txid: id, block: b, amount: delta, log_index: None, fin: false }]
    }

    /// Successful transactions in block `b` that send the native coin straight to `to`.
    async fn direct_transfers(&self, to: Address, b: u64) -> Result<Vec<(String, U256)>> {
        let block = self
            .provider
            .get_block_by_number(BlockNumberOrTag::Number(b))
            .full()
            .await
            .map_err(rpc("block"))?
            .ok_or_else(|| Error::Driver(format!("block {b} not found")))?;
        let mut out = Vec::new();
        for tx in block.transactions.txns() {
            if tx.to() != Some(to) || tx.value().is_zero() {
                continue;
            }
            let hash = tx.inner.tx_hash();
            if self.receipt(*hash).await?.is_some_and(|r| r.status()) {
                out.push((format!("{hash:#x}"), tx.value()));
            }
        }
        Ok(out)
    }

    /// Native arrivals in `(state.final_to, tip]`: the window arrivals that still hold, then new ones.
    async fn native_fresh(&self, state: &ScanState, deposit: Address, flushes: &[(u64, U256)], tip: u64) -> Result<Vec<Arrival>> {
        let flushed = Flushed { base: state.final_flushed, recent: flushes };
        let final_received = state.final_received();
        let window: Vec<&Arrival> = state.arrivals.iter().filter(|a| !a.fin).collect();
        let mut fresh: Vec<Arrival> = Vec::new();
        let mut known = final_received;
        // Keep window arrivals while the chain still shows them at their block.
        let mut i = 0;
        while i < window.len() {
            let b = window[i].block;
            let group: Vec<&Arrival> = window[i..].iter().take_while(|a| a.block == b).copied().collect();
            let sum = group.iter().fold(U256::ZERO, |s, a| s.saturating_add(a.amount));
            if b > tip || self.native_total(deposit, b, &flushed).await? < known.saturating_add(sum) {
                break;
            }
            fresh.extend(group.into_iter().cloned());
            known = known.saturating_add(sum);
            i += window[i..].iter().take_while(|a| a.block == b).count();
        }
        let total_tip = self.native_total(deposit, tip, &flushed).await?;
        if total_tip < known {
            tracing::warn!(network = %self.code, address = %deposit, "native total went down; reading the window again");
            fresh.clear();
            known = final_received;
            if total_tip < known {
                return Ok(fresh);
            }
        }
        let floor = state.final_to.max(tip.saturating_sub(STATE_SPAN));
        let mut lo = fresh.last().map_or(floor, |a| a.block.max(floor));
        for _ in 0..8 {
            if total_tip <= known {
                break;
            }
            let (b, total_b) = self.first_above(deposit, lo, tip, known, total_tip, &flushed).await;
            fresh.extend(self.native_arrivals(deposit, b, total_b - known).await);
            (known, lo) = (total_b, b);
        }
        if total_tip > known {
            fresh.extend(self.native_arrivals(deposit, tip, total_tip - known).await);
        }
        Ok(fresh)
    }

    /// Factory, cold wallet and first block of a deposit address. Addresses without a row are watched from now.
    async fn deposit_info(&self, w: &WalletContext, deposit: Address, tip: u64) -> Result<(Address, u64)> {
        if let Some(d) = self.store.evm_deposit(self.network_id, &key(&deposit)).await? {
            let factory = Address::from_str(&d.factory).map_err(rpc("factory"))?;
            return Ok((factory, d.from_block.max(0) as u64));
        }
        let cold = w
            .address
            .as_deref()
            .and_then(parse_address)
            .ok_or_else(|| Error::Driver(format!("{}: no cold wallet for {deposit}", self.code)))?;
        let factory = evm_factory::factory_address(cold);
        Ok((factory, tip))
    }
}

/// Native coin flushed from one deposit address: the final sum plus the recent `Flushed` logs.
struct Flushed<'a> {
    base: U256,
    recent: &'a [(u64, U256)],
}

impl Flushed<'_> {
    fn at(&self, b: u64) -> U256 {
        self.recent.iter().filter(|(fb, _)| *fb <= b).fold(self.base, |s, (_, a)| s.saturating_add(*a))
    }
}

#[async_trait]
impl PaymentDriver for EvmDriver {
    fn chain(&self) -> &str {
        &self.chain
    }

    fn index_scope(&self) -> IndexScope {
        IndexScope::Family
    }

    fn needs_sweep(&self) -> bool {
        // The EVM sweeper flushes deposits in batches.
        false
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    async fn validate_address(&self, address: &str) -> Result<bool> {
        Ok(parse_address(address).is_some())
    }

    async fn prepare_wallet(&self, w: &WalletContext) -> Result<Vec<String>> {
        let cold =
            w.address.as_deref().and_then(parse_address).ok_or_else(|| Error::BadRequest("This is not a valid EVM address.".into()))?;
        let factory = evm_factory::factory_address(cold);
        self.store.set_project_wallet_keys(w.project_id, self.network_id, None, None, None, Some(&factory.to_checksum(None))).await?;
        if self.has_code(factory).await? {
            return Ok(vec![format!("{}: factory deployed.", self.code)]);
        }
        let data = evm_factory::deploy_factory_tx(cold).input.input().cloned().unwrap_or_default();
        let hex = alloy::primitives::hex::encode(&data);
        Ok(vec![format!(
            "{}: factory not deployed yet: Turmite deploys it with the hot wallet before the first sweep, or anyone can \
             deploy it by sending this transaction: to {}, data 0x{}…{} ({} bytes, in full on the Wallets page).",
            self.code,
            evm_factory::CREATE2_DEPLOYER.to_checksum(None),
            &hex[..16],
            &hex[hex.len() - 8..],
            data.len()
        )])
    }

    async fn derive_deposit(&self, w: &WalletContext, derivation_index: u64) -> Result<DerivedAddress> {
        let cold = w
            .address
            .as_deref()
            .and_then(parse_address)
            .ok_or_else(|| Error::Unavailable("Set the EVM cold wallet on the Wallets page first.".into()))?;
        let factory = evm_factory::factory_address(cold);
        let address = evm_factory::predict_deposit_address(
            factory,
            evm_factory::implementation_address(factory),
            evm_factory::salt_for_index(derivation_index),
        );
        let tip = self.tip().await?;
        self.store
            .insert_evm_deposit(self.network_id, &key(&address), &key(&factory), &key(&cold), derivation_index as i64, tip as i64)
            .await?;
        Ok(DerivedAddress { address: address.to_checksum(None), payin_extra_id: None, derivation_index })
    }

    async fn observe(
        &self,
        w: &WalletContext,
        address: &str,
        _payin_extra_id: Option<&str>,
        asset: &AssetContext,
    ) -> Result<Vec<Observation>> {
        let deposit = Address::from_str(address).map_err(rpc("deposit address"))?;
        let dkey = key(&deposit);
        let tip = self.tip().await?;
        let (factory, from_block) = self.deposit_info(w, deposit, tip).await?;
        let token = match &asset.contract_address {
            Some(c) => Some(Address::from_str(c).map_err(rpc("token contract"))?),
            None => None,
        };
        let asset_key = token.as_ref().map(key).unwrap_or_default();
        let mut state = match self.store.evm_scan(self.network_id, &dkey, &asset_key).await? {
            Some(row) => ScanState {
                final_to: row.final_to.max(0) as u64,
                final_flushed: U256::from_str(&row.final_flushed).unwrap_or_default(),
                arrivals: serde_json::from_value(row.arrivals).map_err(|e| Error::Internal(format!("evm scan state: {e}")))?,
            },
            None => ScanState::new(from_block.saturating_sub(1)),
        };
        let mut gone = Vec::new();
        if tip > state.final_to {
            let from = state.final_to + 1;
            match token {
                Some(token) => {
                    let fresh = self.token_arrivals(token, deposit, from, tip).await?;
                    gone = state.merge(fresh, tip, self.depth);
                }
                None => {
                    let flushes = self.native_flushes(factory, deposit, from, tip).await?;
                    let fresh = self.native_fresh(&state, deposit, &flushes, tip).await?;
                    let old_final_to = state.final_to;
                    gone = state.merge(fresh, tip, self.depth);
                    state.final_flushed = flushes
                        .iter()
                        .filter(|(b, _)| *b > old_final_to && *b <= state.final_to)
                        .fold(state.final_flushed, |s, (_, a)| s.saturating_add(*a));
                }
            }
            let arrivals = serde_json::to_value(&state.arrivals).map_err(|e| Error::Internal(e.to_string()))?;
            self.store
                .save_evm_scan(self.network_id, &dkey, &asset_key, state.final_to as i64, &state.final_flushed.to_string(), &arrivals)
                .await?;
        }
        Ok(state.arrivals.iter().map(|a| observation(a, tip, asset.decimals)).chain(gone.iter().map(removed)).collect())
    }

    async fn sweep(&self, _w: &WalletContext, _req: &SweepRequest, _asset: &AssetContext) -> Result<SweepResult> {
        Err(Error::Driver("EVM deposits are swept in batches by the worker".into()))
    }

    async fn tip_height(&self) -> Result<i64> {
        Ok(self.tip().await? as i64)
    }
}

#[cfg(test)]
mod tests {
    use alloy::primitives::address;

    use super::*;

    fn arrival(key: &str, block: u64, amount: u64) -> Arrival {
        Arrival { key: key.into(), txid: key.into(), block, amount: U256::from(amount), log_index: None, fin: false }
    }

    #[test]
    fn eip55() {
        let good = "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed";
        assert_eq!(parse_address(good), Some(address!("5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed")));
        assert!(parse_address(&good.to_ascii_lowercase()).is_some());
        assert!(parse_address(&format!("0x{}", good[2..].to_ascii_uppercase())).is_some());
        assert!(parse_address("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAeD").is_none(), "wrong checksum");
        assert!(parse_address("5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed").is_none());
        assert!(parse_address("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAe").is_none());
        assert!(parse_address("0x0000000000000000000000000000000000000000").is_none());
    }

    #[test]
    fn amounts() {
        assert_eq!(to_decimal(U256::from(1_500_000u64), 6), Decimal::new(15, 1));
        assert_eq!(to_decimal(U256::from(10u64).pow(U256::from(18)), 18), Decimal::ONE);
        assert_eq!(to_raw(Decimal::new(15, 1), 6), U256::from(1_500_000u64));
        assert_eq!(to_raw(Decimal::new(123, 3), 18), U256::from(123_000_000_000_000_000u64));
    }

    #[test]
    fn merge_finalizes_and_drops_reorged_arrivals() {
        let mut s = ScanState::new(99);
        assert!(s.merge(vec![arrival("a", 100, 5), arrival("b", 110, 7)], 110, 12).is_empty());
        assert_eq!(s.final_to, 99, "nothing is deep enough yet");
        // Next pass: `b` is gone after a reorg, `c` is new, `a` is now final.
        let gone = s.merge(vec![arrival("a", 100, 5), arrival("c", 113, 1)], 113, 12);
        assert_eq!(gone.iter().map(|a| a.key.as_str()).collect::<Vec<_>>(), ["b"]);
        assert_eq!(s.final_to, 101);
        assert!(s.arrivals.iter().find(|a| a.key == "a").unwrap().fin);
        assert!(!s.arrivals.iter().find(|a| a.key == "c").unwrap().fin);
        assert_eq!(s.final_received(), U256::from(5u64));
        // Final arrivals stay when the window is read again.
        let gone = s.merge(vec![], 200, 12);
        assert_eq!(gone.len(), 1);
        assert_eq!(s.arrivals.len(), 1);
        assert_eq!(s.final_to, 188);
    }

    #[test]
    fn observations_count_confirmations_from_the_block() {
        let a = arrival("0xabc", 100, 2_000_000);
        let o = observation(&a, 102, 6);
        assert_eq!((o.confirmations, o.amount, o.block_height), (3, Decimal::from(2), Some(100)));
        assert_eq!(observation(&a, 99, 6).confirmations, 0);
        assert_eq!(removed(&a).amount, Decimal::ZERO);
    }

    #[test]
    fn state_serializes() {
        let a = Arrival { fin: true, log_index: Some(3), ..arrival("k", 7, 9) };
        let v = serde_json::to_value(vec![a.clone()]).unwrap();
        assert_eq!(serde_json::from_value::<Vec<Arrival>>(v).unwrap(), vec![a]);
    }
}
