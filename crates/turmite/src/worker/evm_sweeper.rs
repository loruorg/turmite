//! EVM sweeper: moves the funds at deposit contracts to the cold wallet in batches.
//!
//! Per project, network and factory it takes the payments with credited funds that are not swept and sends
//! one `flush(salts, tokens)` from the project's EVM hot wallet. It deploys the factory first when the chain
//! has no code at its address. The hot key only pays gas: `flush` can only send to the cold wallet fixed in
//! the factory.
//!
//! A batch goes out when gas is at most 2% of its value, when it is worth 1000 USD or more, when it is
//! full, or when its oldest payment waited 6 hours. It never goes out while gas is more than 20% of its
//! value: the funds stay at the deposit addresses, which can only pay the cold wallet.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    str::FromStr,
    time::Duration,
};

use alloy::{
    primitives::{Address, B256, Bytes, U256},
    rpc::types::TransactionReceipt,
    signers::local::PrivateKeySigner,
};
use chrono::Utc;
use rust_decimal::Decimal;

use crate::{
    app::App,
    chains::{
        evm::{self, EvmDriver},
        evm_factory::{self, CREATE2_DEPLOYER, DepositFactory, MAX_SALTS_PER_FLUSH, NATIVE},
    },
    db::{EvmSweepCandidate, EvmTxRow, PgStore},
    domain::{AssetNetwork, Network},
    error::{Error, Result},
    hot::HotKey,
};

const PASS_INTERVAL: Duration = Duration::from_secs(30);
/// Flush at once when gas is at most this part of the value.
const CHEAP: Decimal = Decimal::from_parts(2, 0, 0, false, 2);
/// Never flush when gas is more than this part of the value.
const MAX_GAS_SHARE: Decimal = Decimal::from_parts(20, 0, 0, false, 2);
/// Flush a batch worth this much (USD) even when gas is above `CHEAP`.
const BIG_BATCH_USD: i64 = 1_000;
/// Flush a batch whose oldest payment waited this long.
const MAX_WAIT_HOURS: i64 = 6;
const RECEIPT_WAIT: Duration = Duration::from_secs(45);
/// A broadcast transaction that the node no longer knows after this long counts as dropped.
const DROPPED_MINUTES: i64 = 15;
/// Gas guess per salt and per token for a flush before the factory exists (a first flush also deploys the clone).
const GUESS_GAS_PER_SALT: u64 = 100_000;
const GUESS_GAS_PER_TOKEN: u64 = 30_000;

pub fn spawn(app: App) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(PASS_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            pass(&app).await;
        }
    });
}

/// One pass over every connected EVM network.
pub async fn pass(app: &App) {
    if app.hot_identity().is_none() {
        return;
    }
    let Ok(networks) = app.store.list_networks().await else { return };
    let drivers = app.drivers();
    for n in networks.iter().filter(|n| n.family == "evm" && n.is_enabled) {
        let Some(d) = drivers.get(&n.chain) else { continue };
        let Some(drv) = d.as_any().and_then(|a| a.downcast_ref::<EvmDriver>()) else { continue };
        if let Err(e) = run(app, n, drv, None).await {
            tracing::warn!(network = %n.code, error = %e, "EVM sweep pass failed");
        }
    }
}

/// Sweep a project's deposits on one network now, without waiting for a batch to fill.
/// The 20% gas rule still applies. Returns the flush transaction hashes.
pub async fn sweep_now(app: &App, project_id: i64, network_id: i64) -> Result<Vec<String>> {
    let n = app.store.network(network_id).await?;
    let drivers = app.drivers();
    let d = drivers.get(&n.chain).ok_or_else(|| Error::Unavailable(format!("{} is not connected.", n.name)))?;
    let drv = d
        .as_any()
        .and_then(|a| a.downcast_ref::<EvmDriver>())
        .ok_or_else(|| Error::Unavailable(format!("{} has no EVM driver.", n.name)))?;
    run(app, &n, drv, Some(project_id)).await
}

async fn run(app: &App, n: &Network, drv: &EvmDriver, now_for: Option<i64>) -> Result<Vec<String>> {
    reconcile(app, n, drv).await?;
    send_queued_deploys(app, n, drv).await?;
    let assets: Vec<AssetNetwork> =
        app.store.list_asset_networks().await?.into_iter().filter(|a| a.network.network_id == n.network_id).collect();
    let mut groups: BTreeMap<(i64, String), Vec<EvmSweepCandidate>> = BTreeMap::new();
    for c in app.store.evm_sweep_candidates(n.network_id, now_for).await? {
        groups.entry((c.project_id, c.factory.clone())).or_default().push(c);
    }
    let mut sent = Vec::new();
    for ((project_id, factory), batch) in groups {
        match flush_batch(app, n, drv, &assets, project_id, batch, now_for.is_some()).await {
            Ok(Some(txid)) => sent.push(txid),
            Ok(None) => {}
            Err(e) => tracing::warn!(network = %n.code, project_id, factory, error = %e, "EVM flush failed; retrying later"),
        }
    }
    Ok(sent)
}

fn parse(a: &str) -> Result<Address> {
    Address::from_str(a).map_err(|e| Error::Internal(format!("bad address {a}: {e}")))
}

fn signer_for(key: &HotKey) -> Result<PrivateKeySigner> {
    let signer = PrivateKeySigner::from_slice(&key.secret).map_err(|e| Error::Internal(format!("EVM hot key: {e}")))?;
    if Some(signer.address()) != Address::from_str(&key.address).ok() {
        return Err(Error::Internal(format!("hot wallet {} does not match its key", key.hot_wallet_id)));
    }
    Ok(signer)
}

/// Settle broadcast transactions that have a receipt; fail the ones the node dropped.
async fn reconcile(app: &App, n: &Network, drv: &EvmDriver) -> Result<()> {
    for t in app.store.open_evm_txs(n.network_id).await?.into_iter().filter(|t| t.status == "broadcast") {
        let Some(hash) = t.txid.as_deref().and_then(|h| B256::from_str(h).ok()) else { continue };
        match drv.receipt(hash).await? {
            Some(r) => settle(app, n, &t, &r).await?,
            None if t.updated_at < Utc::now() - chrono::Duration::minutes(DROPPED_MINUTES) && !drv.tx_known(hash).await? => {
                fail(app, n, &t, "dropped by the node").await?;
            }
            None => {}
        }
    }
    Ok(())
}

async fn fail(app: &App, n: &Network, t: &EvmTxRow, reason: &str) -> Result<()> {
    tracing::warn!(network = %n.code, txid = ?t.txid, reason, "EVM transaction failed");
    app.store.finish_evm_tx(t.evm_tx_id, "failed", None, None, Some(reason)).await?;
    if let Some(txid) = &t.txid {
        for s in app.store.evm_sweeps_for_tx(n.network_id, txid).await? {
            app.store.settle_evm_sweep(s.sweep_id, "failed", None, Some(reason)).await?;
        }
    }
    Ok(())
}

/// Record a mined transaction. For a flush: the amount that left each deposit, per asset.
async fn settle(app: &App, n: &Network, t: &EvmTxRow, r: &TransactionReceipt) -> Result<()> {
    if !r.status() {
        return fail(app, n, t, "reverted").await;
    }
    let fee = U256::from(r.gas_used) * U256::from(r.effective_gas_price);
    app.store.finish_evm_tx(t.evm_tx_id, "confirmed", Some(r.gas_used as i64), Some(&fee.to_string()), None).await?;
    tracing::info!(network = %n.code, kind = %t.kind, txid = ?t.txid, gas = r.gas_used, salts = t.salts, "EVM transaction confirmed");
    let (Some(txid), "flush") = (t.txid.as_deref(), t.kind.as_str()) else { return Ok(()) };
    let factory = parse(&t.factory)?;
    let mut flushed: HashMap<(Address, Address), U256> = HashMap::new();
    let mut refused: HashSet<(Address, Address)> = HashSet::new();
    for log in r.logs().iter().filter(|l| l.address() == factory) {
        if let Ok(e) = log.log_decode::<DepositFactory::Flushed>() {
            let e = e.inner.data;
            *flushed.entry((e.wallet, e.token)).or_default() += e.amount;
        } else if let Ok(e) = log.log_decode::<DepositFactory::FlushFailed>() {
            refused.insert((e.inner.data.wallet, e.inner.data.token));
        }
    }
    let mut matched = HashSet::new();
    for s in app.store.evm_sweeps_for_tx(n.network_id, txid).await? {
        let token = match &s.contract_address {
            Some(c) => parse(c)?,
            None => NATIVE,
        };
        let k = (parse(&s.from_address)?, token);
        matched.insert(k);
        if refused.contains(&k) {
            tracing::warn!(network = %n.code, deposit = %s.from_address, token = %token, "the token refused the transfer; retrying later");
            app.store.settle_evm_sweep(s.sweep_id, "failed", None, Some("the token refused the transfer")).await?;
        } else {
            let amount = evm::to_decimal(flushed.get(&k).copied().unwrap_or_default(), s.decimals as u32);
            app.store.settle_evm_sweep(s.sweep_id, "confirmed", Some(amount), None).await?;
        }
    }
    // Other assets that left the deposits (for example the native coin sent to a token payment's address).
    let extra: Vec<_> = flushed.into_iter().filter(|(k, a)| !matched.contains(k) && !a.is_zero()).collect();
    if !extra.is_empty() {
        let assets = app.store.list_asset_networks().await?;
        for ((wallet, token), amount) in extra {
            let an = assets.iter().find(|a| {
                a.network.network_id == n.network_id
                    && match &a.contract_address {
                        None => token == NATIVE,
                        Some(c) => Address::from_str(c).ok() == Some(token),
                    }
            });
            let (Some(an), Some(project_id)) = (an, t.project_id) else {
                tracing::info!(network = %n.code, deposit = %wallet, token = %token, %amount, "flushed an asset that is not in the catalog");
                continue;
            };
            let amount = evm::to_decimal(amount, an.decimals as u32);
            let mut conn = app.store.pool.acquire().await?;
            PgStore::record_evm_sweep(
                &mut conn,
                project_id,
                n.network_id,
                an.asset_network_id,
                None,
                &wallet.to_checksum(None),
                &t.cold_wallet,
                amount,
                "confirmed",
                txid,
            )
            .await?;
        }
    }
    Ok(())
}

/// Send the deployments that operators asked for in the dashboard.
async fn send_queued_deploys(app: &App, n: &Network, drv: &EvmDriver) -> Result<()> {
    for t in app.store.open_evm_txs(n.network_id).await?.into_iter().filter(|t| t.status == "queued" && t.kind == "deploy") {
        let factory = parse(&t.factory)?;
        if drv.has_code(factory).await? {
            app.store.finish_evm_tx(t.evm_tx_id, "confirmed", None, None, Some("already deployed")).await?;
            continue;
        }
        let key = match t.project_id {
            Some(pid) => crate::hot::key_for_project(app, pid, "evm").await?,
            None => None,
        };
        let Some(key) = key else {
            app.store.finish_evm_tx(t.evm_tx_id, "failed", None, None, Some("the project has no EVM hot wallet")).await?;
            continue;
        };
        let result = async {
            let signer = signer_for(&key)?;
            deploy(app, n, drv, &signer, t.project_id.unwrap_or_default(), factory, parse(&t.cold_wallet)?, Some(t.evm_tx_id)).await
        }
        .await;
        if let Err(e) = result {
            tracing::warn!(network = %n.code, factory = %factory, error = %e, "factory deployment failed; retrying later");
            app.store.finish_evm_tx(t.evm_tx_id, "queued", None, None, Some(&e.to_string())).await?;
        }
    }
    Ok(())
}

/// What a broadcast writes in the same database transaction as the nonce lock.
enum Record<'a> {
    Deploy { queued_id: Option<i64> },
    Flush { plans: &'a [Plan] },
}

/// Under the nonce lock: take the next nonce, sign, record, broadcast. Returns the hash.
#[allow(clippy::too_many_arguments)]
async fn send(
    app: &App,
    n: &Network,
    drv: &EvmDriver,
    signer: &PrivateKeySigner,
    project_id: i64,
    factory: Address,
    cold: Address,
    to: Address,
    data: Bytes,
    gas: u64,
    record: Record<'_>,
) -> Result<B256> {
    let fees = drv.fees().await?;
    let from = signer.address();
    let mut db = app.store.pool.begin().await?;
    PgStore::lock_evm_nonce(&mut db, &n.chain, &from.to_checksum(None)).await?;
    let nonce = PgStore::next_evm_nonce(&mut db, n.network_id, &from.to_checksum(None), drv.pending_nonce(from).await?).await?;
    let (hash, raw) = drv.sign(signer, nonce, to, data, gas, fees).await?;
    let txid = format!("{hash:#x}");
    let (kind, queued_id, salts) = match &record {
        Record::Deploy { queued_id } => ("deploy", *queued_id, 0),
        Record::Flush { plans } => ("flush", None, plans.len() as i32),
    };
    let cold_s = cold.to_checksum(None);
    PgStore::record_evm_tx(
        &mut db,
        queued_id,
        n.network_id,
        project_id,
        kind,
        &evm::key(&factory),
        &cold_s,
        &from.to_checksum(None),
        nonce as i64,
        &txid,
        salts,
    )
    .await?;
    if let Record::Flush { plans } = record {
        for p in plans {
            PgStore::record_evm_sweep(
                &mut db,
                project_id,
                n.network_id,
                p.c.asset_network_id,
                Some(p.c.payment_id),
                &p.c.deposit,
                &cold_s,
                p.c.available,
                "broadcast",
                &txid,
            )
            .await?;
        }
    }
    drv.broadcast(&raw).await?;
    db.commit().await?;
    Ok(hash)
}

/// Deploy a factory through the CREATE2 deployer and wait for its receipt.
#[allow(clippy::too_many_arguments)]
async fn deploy(
    app: &App,
    n: &Network,
    drv: &EvmDriver,
    signer: &PrivateKeySigner,
    project_id: i64,
    factory: Address,
    cold: Address,
    queued_id: Option<i64>,
) -> Result<()> {
    let data = evm_factory::deploy_factory_tx(cold).input.input().cloned().unwrap_or_default();
    let gas = drv.estimate_gas(signer.address(), CREATE2_DEPLOYER, data.clone()).await?;
    let gas = gas + gas / 5;
    let hash = send(app, n, drv, signer, project_id, factory, cold, CREATE2_DEPLOYER, data, gas, Record::Deploy { queued_id }).await?;
    tracing::info!(network = %n.code, factory = %factory, txid = %hash, "factory deployment sent");
    if let Some(r) = drv.wait_receipt(hash, RECEIPT_WAIT).await?
        && let Some(t) = app.store.evm_tx_by_txid(n.network_id, &format!("{hash:#x}")).await?
    {
        settle(app, n, &t, &r).await?;
    }
    Ok(())
}

/// One deposit in a batch.
struct Plan {
    c: EvmSweepCandidate,
}

async fn usd(app: &App, coingecko_id: Option<&str>) -> Option<Decimal> {
    app.rates.price(coingecko_id?, "usd").await.ok().filter(|p| *p > Decimal::ZERO)
}

/// Build, price and maybe send one flush for a project's deposits of one factory.
async fn flush_batch(
    app: &App,
    n: &Network,
    drv: &EvmDriver,
    assets: &[AssetNetwork],
    project_id: i64,
    batch: Vec<EvmSweepCandidate>,
    now: bool,
) -> Result<Option<String>> {
    let Some(first) = batch.first() else { return Ok(None) };
    let (factory, cold) = (parse(&first.factory)?, parse(&first.cold_wallet)?);
    let Some(key) = crate::hot::key_for_project(app, project_id, "evm").await? else {
        tracing::info!(network = %n.code, project_id, "no EVM hot wallet; deposits wait at their addresses");
        return Ok(None);
    };
    let signer = signer_for(&key)?;
    let from = signer.address();

    // Keep the deposits that still hold the payment's asset.
    let mut plans = Vec::new();
    for c in batch {
        if plans.len() == MAX_SALTS_PER_FLUSH {
            break;
        }
        let deposit = parse(&c.deposit)?;
        let balance = match &c.contract_address {
            Some(t) => drv.token_balance(parse(t)?, deposit).await?,
            None => drv.balance(deposit).await?,
        };
        if balance.is_zero() {
            tracing::info!(network = %n.code, payment_id = c.payment_id, "deposit already empty; counted as swept");
            app.store.mark_evm_payment_swept(c.payment_id).await?;
            continue;
        }
        plans.push(Plan { c });
    }
    if plans.is_empty() {
        return Ok(None);
    }
    let salts: Vec<B256> = plans.iter().map(|p| evm_factory::salt_for_index(p.c.salt_index as u64)).collect();
    let mut tokens = vec![NATIVE];
    for p in &plans {
        if let Some(t) = &p.c.contract_address {
            let t = parse(t)?;
            if !tokens.contains(&t) {
                tokens.push(t);
            }
        }
    }
    let data = evm_factory::flush_call(&salts, &tokens);

    let (max_fee, _) = match drv.fees().await {
        Ok(f) => f,
        Err(e) => {
            tracing::info!(network = %n.code, error = %e, "EVM sweep waits for lower fees");
            return Ok(None);
        }
    };
    let deployed = drv.has_code(factory).await?;
    if !deployed && app.store.evm_deploy_in_flight(n.network_id, &evm::key(&factory)).await? {
        return Ok(None);
    }
    let gas = if deployed {
        drv.estimate_gas(from, factory, data.clone()).await?
    } else {
        let deploy_data = evm_factory::deploy_factory_tx(cold).input.input().cloned().unwrap_or_default();
        drv.estimate_gas(from, CREATE2_DEPLOYER, deploy_data).await?
            + plans.len() as u64 * (GUESS_GAS_PER_SALT + GUESS_GAS_PER_TOKEN * tokens.len() as u64)
    };
    let cost_wei = U256::from(gas) * U256::from(max_fee);
    let native = assets.iter().find(|a| a.is_native());
    let native_decimals = native.map_or(18, |a| a.decimals as u32);
    let cost = evm::to_decimal(cost_wei, native_decimals);

    // Gas against value, in USD; without prices only a native-coin batch can be judged.
    let mut value_usd = Some(Decimal::ZERO);
    for p in &plans {
        value_usd = match (value_usd, usd(app, p.c.coingecko_id.as_deref()).await) {
            (Some(v), Some(price)) => Some(v + p.c.available * price),
            _ => None,
        };
    }
    let native_usd = usd(app, native.and_then(|a| a.asset.coingecko_id.as_deref())).await;
    let share = match (value_usd, native_usd) {
        (Some(v), Some(price)) if v > Decimal::ZERO => Some(cost * price / v),
        _ if plans.iter().all(|p| p.c.contract_address.is_none()) => {
            let v: Decimal = plans.iter().map(|p| p.c.available).sum();
            (v > Decimal::ZERO).then(|| cost / v)
        }
        _ => None,
    };
    let Some(share) = share else {
        tracing::warn!(network = %n.code, project_id, "no price for this batch; the EVM sweep waits");
        return Ok(None);
    };
    if share > MAX_GAS_SHARE {
        tracing::info!(
            network = %n.code, project_id, deposits = plans.len(), gas_share = %share.round_dp(3),
            "EVM sweep waits: gas is more than 20% of the value; the funds stay at the deposit addresses"
        );
        return Ok(None);
    }
    let oldest = plans.iter().filter_map(|p| p.c.paid_at).min();
    let due = now
        || share <= CHEAP
        || value_usd.is_some_and(|v| v >= Decimal::from(BIG_BATCH_USD))
        || plans.len() >= MAX_SALTS_PER_FLUSH
        || oldest.is_some_and(|t| t < Utc::now() - chrono::Duration::hours(MAX_WAIT_HOURS));
    if !due {
        return Ok(None);
    }
    let hot_balance = drv.balance(from).await?;
    if hot_balance < cost_wei {
        tracing::warn!(network = %n.code, hot_wallet = %from, need = %cost, "the EVM hot wallet needs more native coin for gas");
        return Ok(None);
    }
    if !deployed {
        deploy(app, n, drv, &signer, project_id, factory, cold, None).await?;
        if !drv.has_code(factory).await? {
            return Ok(None);
        }
    }
    let gas = drv.estimate_gas(from, factory, data.clone()).await?;
    let gas = gas + gas / 4 + 10_000;
    let hash = send(app, n, drv, &signer, project_id, factory, cold, factory, data, gas, Record::Flush { plans: &plans }).await?;
    let txid = format!("{hash:#x}");
    tracing::info!(network = %n.code, project_id, txid, deposits = plans.len(), "EVM flush sent");
    if let Some(r) = drv.wait_receipt(hash, RECEIPT_WAIT).await?
        && let Some(t) = app.store.evm_tx_by_txid(n.network_id, &txid).await?
    {
        settle(app, n, &t, &r).await?;
    }
    Ok(Some(txid))
}
