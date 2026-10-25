//! TRON driver over the full node HTTP API: TRX and TRC-20 tokens.
//!
//! Deposit key per payment: secp256k1 from the deposit seed and the index
//! (address = base58check(0x41 ++ keccak256(pubkey)[12..])). The driver encodes and
//! signs its transactions itself, so a node cannot change what is signed.
//!
//! Incoming transfers come from the TronGrid-style `/v1/accounts` API when the node
//! has it. A plain full node has no history by address: then the driver reads the
//! balance (`getaccount`, `balanceOf`), which does not see a top-up after a sweep.
//! A transfer counts as final when its block is solidified.
//!
//! TRC-20 sweep: the project's hot wallet sends the deposit address the TRX that the
//! token transfer burns for energy and bandwidth, then the deposit calls `transfer`.

use std::{collections::HashMap, sync::Mutex, time::Duration};

use async_trait::async_trait;
use k256::ecdsa::SigningKey;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sha3::Keccak256;
use zeroize::Zeroizing;

use crate::{
    chains::driver::{
        AssetContext, DerivedAddress, Observation, PaymentDriver, SweepCheck, SweepRequest, SweepResult, SweepState, WalletContext,
    },
    error::{Error, Result},
};

const SUN_PER_TRX: u64 = 1_000_000;
const TRANSFER_CONTRACT: u64 = 1;
const CREATE_SMART_CONTRACT: u64 = 30;
const TRIGGER_SMART_CONTRACT: u64 = 31;
/// Bandwidth (bytes) of a signed TRX transfer and of a signed TRC-20 transfer, rounded up.
const TRX_TX_BYTES: u64 = 300;
const TRC20_TX_BYTES: u64 = 350;
/// A transaction expires one minute after it is made.
const EXPIRATION_MS: i64 = 60_000;
/// The zero address: a valid account that nobody controls.
const BLACK_HOLE: &str = "T9yD14Nj9j7xAB4dbGeiX9h8unkKHxuWwb";
const TRANSFER_SELECTOR: &str = "transfer(address,uint256)";
/// Block 0 ids of the public networks.
const MAINNET_GENESIS: &str = "00000000000000001ebf88508a03865c71d452e25f4d51194196a1d22b6653dc";
const NILE_GENESIS: &str = "0000000000000000d698d4192c56cb6be724a558448e2684802de4d6cd8690dc";
const SHASTA_GENESIS: &str = "0000000000000000de1aa88295e1fcf982742f773e0419c5a9c134c994a9059e";

pub struct TronDriver {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    master_seed: Option<Zeroizing<String>>,
    confirmations_required: i32,
    /// The node serves the TronGrid-style `/v1/accounts` history API.
    indexed: bool,
    /// Block of transactions that are solidified (it cannot change).
    blocks: Mutex<HashMap<String, i64>>,
}

/// Where a transaction is.
enum TxState {
    Pending,
    Failed,
    InBlock(i64),
}

/// The block a new transaction refers to (TaPoS).
struct RefBlock {
    bytes: [u8; 2],
    hash: [u8; 8],
    timestamp: i64,
}

struct ChainParams {
    /// Sun per energy unit.
    energy_fee: u64,
    /// Sun per bandwidth byte.
    byte_fee: u64,
    /// Sun to create an account with a TRX transfer.
    create_account_fee: u64,
}

impl TronDriver {
    /// `url` is the node's HTTP root. An `apikey` query parameter is sent as the
    /// `TRON-PRO-API-KEY` header. Without `master_seed` (the API process), the driver
    /// watches and validates but cannot derive or sweep. Refuses a node on another
    /// network than the mode's (see `check_genesis`).
    pub async fn connect(url: &str, master_seed: Option<&str>, testnet: bool, confirmations_required: i32) -> Result<Self> {
        let (base_url, api_key) = crate::chains::registry::split_api_key(url);
        let mut d = Self {
            client: reqwest::Client::builder().timeout(Duration::from_secs(20)).build().map_err(|e| Error::Internal(e.to_string()))?,
            base_url,
            api_key,
            master_seed: master_seed.map(|s| Zeroizing::new(s.to_string())),
            confirmations_required,
            indexed: false,
            blocks: Mutex::new(HashMap::new()),
        };
        let genesis = d.rpc("/wallet/getblockbynum", json!({ "num": 0 })).await?;
        check_genesis(genesis.get("blockID").and_then(Value::as_str).unwrap_or_default(), testnet)?;
        d.latest_block().await?;
        d.indexed = d.v1(&format!("/v1/accounts/{BLACK_HOLE}/transactions/trc20"), &[("limit", "1")]).await.is_ok();
        Ok(d)
    }

    /// True when the node has the TronGrid-style history API.
    pub fn is_indexed(&self) -> bool {
        self.indexed
    }

    /// Use the history API or balances (tests: behave as a node without the API).
    pub fn set_indexed(&mut self, indexed: bool) {
        self.indexed = indexed;
    }

    async fn rpc(&self, path: &str, body: Value) -> Result<Value> {
        let mut req = self.client.post(format!("{}{}", self.base_url, path)).json(&body);
        if let Some(k) = &self.api_key {
            req = req.header("TRON-PRO-API-KEY", k);
        }
        let resp = req.send().await.map_err(|e| Error::Driver(format!("tron {path}: {e}")))?;
        let status = resp.status();
        let v: Value = resp.json().await.map_err(|e| Error::Driver(format!("tron {path}: HTTP {status}: {e}")))?;
        if let Some(e) = v.get("Error") {
            return Err(Error::Driver(format!("tron {path}: {e}")));
        }
        Ok(v)
    }

    /// GET on the TronGrid-style API. Returns `data`.
    async fn v1(&self, path: &str, query: &[(&str, &str)]) -> Result<Vec<Value>> {
        let mut req = self.client.get(format!("{}{}", self.base_url, path)).query(query);
        if let Some(k) = &self.api_key {
            req = req.header("TRON-PRO-API-KEY", k);
        }
        let resp = req.send().await.map_err(|e| Error::Driver(format!("tron {path}: {e}")))?;
        if !resp.status().is_success() {
            return Err(Error::Driver(format!("tron {path}: HTTP {}", resp.status())));
        }
        let v: Value = resp.json().await.map_err(|e| Error::Driver(format!("tron {path}: {e}")))?;
        if v.get("success").and_then(Value::as_bool) != Some(true) {
            return Err(Error::Driver(format!("tron {path}: {v}")));
        }
        Ok(v.get("data").and_then(Value::as_array).cloned().unwrap_or_default())
    }

    fn seed(&self) -> Result<&str> {
        self.master_seed
            .as_deref()
            .map(|s| s.as_str())
            .ok_or_else(|| Error::Driver("tron: the deposit seed is only available in the worker".into()))
    }

    // ---- chain state -------------------------------------------------------

    async fn now_block(&self, solid: bool) -> Result<Value> {
        let path = if solid { "/walletsolidity/getnowblock" } else { "/wallet/getnowblock" };
        self.rpc(path, json!({})).await
    }

    async fn latest_block(&self) -> Result<i64> {
        block_number(&self.now_block(false).await?)
    }

    async fn solid_block(&self) -> Result<i64> {
        block_number(&self.now_block(true).await?)
    }

    async fn ref_block(&self) -> Result<RefBlock> {
        let b = self.now_block(false).await?;
        let number = block_number(&b)?;
        let id = b.get("blockID").and_then(Value::as_str).and_then(|s| hex::decode(s).ok()).filter(|v| v.len() == 32);
        let id = id.ok_or_else(|| Error::Driver(format!("tron: block without id: {b}")))?;
        let timestamp = b.pointer("/block_header/raw_data/timestamp").and_then(Value::as_i64).unwrap_or(0);
        let mut hash = [0u8; 8];
        hash.copy_from_slice(&id[8..16]);
        Ok(RefBlock { bytes: [(number >> 8) as u8, number as u8], hash, timestamp })
    }

    async fn params(&self) -> Result<ChainParams> {
        let v = self.rpc("/wallet/getchainparameters", json!({})).await?;
        let get = |key: &str| {
            v.get("chainParameter")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|p| p.get("key").and_then(Value::as_str) == Some(key))
                .map(|p| p.get("value").and_then(Value::as_u64).unwrap_or(0))
        };
        Ok(ChainParams {
            energy_fee: get("getEnergyFee").unwrap_or(420),
            byte_fee: get("getTransactionFee").unwrap_or(1000),
            create_account_fee: get("getCreateAccountFee").unwrap_or(100_000)
                + get("getCreateNewAccountFeeInSystemContract").unwrap_or(1_000_000),
        })
    }

    /// (balance in sun, account exists).
    async fn account(&self, addr: &[u8; 21], solid: bool) -> Result<(u64, bool)> {
        let path = if solid { "/walletsolidity/getaccount" } else { "/wallet/getaccount" };
        let v = self.rpc(path, json!({ "address": to_b58(addr), "visible": true })).await?;
        let exists = v.as_object().is_some_and(|o| !o.is_empty());
        Ok((v.get("balance").and_then(Value::as_u64).unwrap_or(0), exists))
    }

    /// (bandwidth bytes, energy) the account can use without burning TRX.
    async fn resources(&self, addr: &[u8; 21]) -> Result<(u64, u64)> {
        let v = self.rpc("/wallet/getaccountresource", json!({ "address": to_b58(addr), "visible": true })).await?;
        let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        let bandwidth = n("freeNetLimit").saturating_sub(n("freeNetUsed")) + n("NetLimit").saturating_sub(n("NetUsed"));
        let energy = n("EnergyLimit").saturating_sub(n("EnergyUsed"));
        Ok((bandwidth, energy))
    }

    async fn trc20_balance(&self, contract: &[u8; 21], owner: &[u8; 21], solid: bool) -> Result<u128> {
        let path = if solid { "/walletsolidity/triggerconstantcontract" } else { "/wallet/triggerconstantcontract" };
        let v = self
            .rpc(
                path,
                json!({
                    "owner_address": to_b58(owner),
                    "contract_address": to_b58(contract),
                    "function_selector": "balanceOf(address)",
                    "parameter": hex::encode(abi_address(owner)),
                    "visible": true,
                }),
            )
            .await?;
        let word = v
            .pointer("/constant_result/0")
            .and_then(Value::as_str)
            .and_then(|s| hex::decode(s).ok())
            .filter(|b| b.len() == 32)
            .ok_or_else(|| Error::Driver(format!("tron balanceOf: {v}")))?;
        if word[..16].iter().any(|b| *b != 0) {
            return Err(Error::Driver("tron balanceOf: balance does not fit in 128 bits".into()));
        }
        Ok(u128::from_be_bytes(word[16..].try_into().expect("16 bytes")))
    }

    async fn tx_info(&self, txid: &str) -> Result<Option<Value>> {
        let v = self.rpc("/wallet/gettransactioninfobyid", json!({ "value": txid })).await?;
        Ok(v.get("id").is_some().then_some(v))
    }

    /// The transaction info, once the transaction is in a block (up to `secs`).
    async fn wait_info(&self, txid: &str, secs: u64) -> Option<Value> {
        let end = tokio::time::Instant::now() + Duration::from_secs(secs);
        while tokio::time::Instant::now() < end {
            if let Ok(Some(info)) = self.tx_info(txid).await {
                return Some(info);
            }
            tokio::time::sleep(Duration::from_millis(1500)).await;
        }
        None
    }

    async fn tx_state(&self, txid: &str, solid: i64) -> Result<TxState> {
        if let Some(b) = self.blocks.lock().unwrap_or_else(|e| e.into_inner()).get(txid) {
            return Ok(TxState::InBlock(*b));
        }
        let Some(info) = self.tx_info(txid).await? else { return Ok(TxState::Pending) };
        let failed = info.get("result").and_then(Value::as_str) == Some("FAILED")
            || info.pointer("/receipt/result").and_then(Value::as_str).is_some_and(|r| r != "SUCCESS");
        if failed {
            return Ok(TxState::Failed);
        }
        let Some(block) = info.get("blockNumber").and_then(Value::as_i64) else { return Ok(TxState::Pending) };
        if block <= solid {
            let mut cache = self.blocks.lock().unwrap_or_else(|e| e.into_inner());
            if cache.len() > 50_000 {
                cache.clear();
            }
            cache.insert(txid.to_string(), block);
        }
        Ok(TxState::InBlock(block))
    }

    /// Blocks from `block` to the tip. Below the requirement until the block is solidified.
    fn confirmations(&self, block: i64, latest: i64, solid: i64) -> i32 {
        let n = (latest - block + 1).clamp(0, i32::MAX as i64) as i32;
        if block > solid { n.min(self.confirmations_required - 1).max(0) } else { n.max(1) }
    }

    // ---- transactions ------------------------------------------------------

    /// `Transaction.raw` referring to the head block.
    async fn make_raw(&self, contract_type: u64, type_name: &str, param: &[u8], fee_limit: u64) -> Result<Vec<u8>> {
        let r = self.ref_block().await?;
        let now = now_ms();
        Ok(raw_transaction(&r, expiration(&r, now), now, contract_type, type_name, param, fee_limit))
    }

    /// Sign `raw` (protobuf `Transaction.raw`) and broadcast it. Returns the txid.
    async fn broadcast(&self, signer: &SigningKey, raw: &[u8]) -> Result<String> {
        let digest = Sha256::digest(raw);
        let txid = hex::encode(digest);
        let (sig, recid) = signer.sign_prehash_recoverable(&digest);
        let mut sig_bytes = sig.to_bytes().to_vec();
        sig_bytes.push(recid.to_byte());
        let mut tx = Vec::with_capacity(raw.len() + 70);
        pb_bytes(&mut tx, 1, raw);
        pb_bytes(&mut tx, 2, &sig_bytes);
        let body = json!({ "transaction": hex::encode(&tx) });
        for _ in 0..6 {
            let res = self.rpc("/wallet/broadcasthex", body.clone()).await?;
            if res.get("result").and_then(Value::as_bool) == Some(true) {
                return Ok(txid);
            }
            let code = res.get("code").and_then(Value::as_str).unwrap_or_default();
            match code {
                "DUP_TRANSACTION_ERROR" => return Ok(txid),
                "SERVER_BUSY" | "BLOCK_UNSOLIDIFIED" | "NOT_ENOUGH_EFFECTIVE_CONNECTION" => {
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                }
                _ => {
                    let msg = res.get("message").and_then(Value::as_str).map(decode_message).unwrap_or_default();
                    return Err(Error::Driver(format!("tron broadcast {code}: {msg}")));
                }
            }
        }
        Err(Error::Driver("tron broadcast: the node stays busy".into()))
    }

    async fn send_trx(&self, signer: &SigningKey, to: &[u8; 21], sun: u64) -> Result<String> {
        let from = address_of(signer);
        let mut param = Vec::new();
        pb_bytes(&mut param, 1, &from);
        pb_bytes(&mut param, 2, to);
        pb_uint(&mut param, 3, sun);
        let raw = self.make_raw(TRANSFER_CONTRACT, "TransferContract", &param, 0).await?;
        self.broadcast(signer, &raw).await
    }

    async fn trigger(&self, signer: &SigningKey, contract: &[u8; 21], data: &[u8], fee_limit: u64) -> Result<String> {
        let from = address_of(signer);
        let mut param = Vec::new();
        pb_bytes(&mut param, 1, &from);
        pb_bytes(&mut param, 2, contract);
        pb_bytes(&mut param, 4, data);
        let raw = self.make_raw(TRIGGER_SMART_CONTRACT, "TriggerSmartContract", &param, fee_limit).await?;
        self.broadcast(signer, &raw).await
    }

    /// Energy that `transfer(to, amount)` from `owner` uses now. A constant call reports
    /// the exact use; `estimateenergy` adds a wide margin and is only the fallback.
    async fn estimate_transfer_energy(&self, owner: &[u8; 21], contract: &[u8; 21], parameter: &str) -> Result<u64> {
        let body = json!({
            "owner_address": to_b58(owner),
            "contract_address": to_b58(contract),
            "function_selector": TRANSFER_SELECTOR,
            "parameter": parameter,
            "visible": true,
        });
        let v = self.rpc("/wallet/triggerconstantcontract", body.clone()).await?;
        if v.pointer("/result/result").and_then(Value::as_bool) != Some(true) {
            let msg = v.pointer("/result/message").and_then(Value::as_str).map(decode_message).unwrap_or_default();
            return Err(Error::Driver(format!("tron: the token transfer would fail: {msg}")));
        }
        // `energy_used` includes the dynamic-energy penalty.
        if let Some(e) = v.get("energy_used").and_then(Value::as_u64).filter(|e| *e > 0) {
            return Ok(e);
        }
        let v = self.rpc("/wallet/estimateenergy", body).await?;
        v.get("energy_required")
            .and_then(Value::as_u64)
            .filter(|e| *e > 0)
            .ok_or_else(|| Error::Driver(format!("tron: no energy estimate: {v}")))
    }

    /// Wait until `addr` holds at least `sun` (head state).
    async fn wait_balance(&self, addr: &[u8; 21], sun: u64, secs: u64) -> Result<()> {
        let end = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            if self.account(addr, false).await.map(|(b, _)| b).unwrap_or(0) >= sun {
                return Ok(());
            }
            if tokio::time::Instant::now() >= end {
                return Err(Error::Driver(format!("tron: the fee transfer to {} has not arrived yet", to_b58(addr))));
            }
            tokio::time::sleep(Duration::from_millis(1500)).await;
        }
    }

    // ---- observe -----------------------------------------------------------

    async fn observe_indexed(&self, address: &str, asset: &AssetContext, latest: i64, solid: i64) -> Result<Vec<Observation>> {
        let addr = parse_address(address)?;
        let mut found: Vec<(String, u128, Option<i64>)> = Vec::new();
        match asset.contract_address.as_deref() {
            Some(contract) => {
                let path = format!("/v1/accounts/{address}/transactions/trc20");
                let query = [("only_to", "true"), ("limit", "50"), ("contract_address", contract)];
                for r in self.v1(&path, &query).await? {
                    let txid = r.get("transaction_id").and_then(Value::as_str).unwrap_or_default();
                    let token = r.pointer("/token_info/address").and_then(Value::as_str).unwrap_or_default();
                    let to = r.get("to").and_then(Value::as_str).unwrap_or_default();
                    let kind = r.get("type").and_then(Value::as_str).unwrap_or("Transfer");
                    let value = r.get("value").and_then(Value::as_str).and_then(|v| v.parse::<u128>().ok()).unwrap_or(0);
                    if txid.is_empty() || token != contract || kind != "Transfer" || value == 0 || !same_address(to, &addr) {
                        continue;
                    }
                    found.push((txid.to_string(), value, None));
                }
            }
            None => {
                let path = format!("/v1/accounts/{address}/transactions");
                for r in self.v1(&path, &[("only_to", "true"), ("limit", "50")]).await? {
                    let txid = r.get("txID").and_then(Value::as_str).unwrap_or_default();
                    let c = r.pointer("/raw_data/contract/0");
                    let kind = c.and_then(|c| c.get("type")).and_then(Value::as_str).unwrap_or_default();
                    let ok = r.pointer("/ret/0/contractRet").and_then(Value::as_str) == Some("SUCCESS");
                    let to = c.and_then(|c| c.pointer("/parameter/value/to_address")).and_then(Value::as_str).unwrap_or_default();
                    let amount = c.and_then(|c| c.pointer("/parameter/value/amount")).and_then(Value::as_u64).unwrap_or(0);
                    if txid.is_empty() || kind != "TransferContract" || !ok || amount == 0 || !same_address(to, &addr) {
                        continue;
                    }
                    found.push((txid.to_string(), amount as u128, r.get("blockNumber").and_then(Value::as_i64)));
                }
            }
        }
        let mut seen: HashMap<String, u32> = HashMap::new();
        let mut out = Vec::new();
        for (txid, units, block) in found {
            let block = match block {
                Some(b) => Some(b),
                None => match self.tx_state(&txid, solid).await? {
                    TxState::Failed => continue,
                    TxState::Pending => None,
                    TxState::InBlock(b) => Some(b),
                },
            };
            let n = seen.entry(txid.clone()).or_insert(0);
            let key = if *n == 0 { txid.clone() } else { format!("{txid}:{n}") };
            *n += 1;
            out.push(Observation {
                chain_tx_key: key,
                txid,
                vout: None,
                amount: units_to_decimal(units, asset.decimals),
                confirmations: block.map(|b| self.confirmations(b, latest, solid)).unwrap_or(0),
                block_height: block,
            });
        }
        Ok(out)
    }

    /// Balance-based detection for nodes without the history API.
    async fn observe_balance(&self, address: &str, asset: &AssetContext) -> Result<Vec<Observation>> {
        let addr = parse_address(address)?;
        let (head, solid, key) = match asset.contract_address.as_deref() {
            Some(contract) => {
                let c = parse_address(contract)?;
                let head = self.trc20_balance(&c, &addr, false).await?;
                let solid = self.trc20_balance(&c, &addr, true).await.unwrap_or(0);
                (head, solid, format!("{address}-{contract}-balance"))
            }
            None => {
                let head = self.account(&addr, false).await?.0 as u128;
                let solid = self.account(&addr, true).await?.0 as u128;
                (head, solid, format!("{address}-balance"))
            }
        };
        if head == 0 {
            return Ok(vec![]);
        }
        Ok(vec![Observation {
            chain_tx_key: key.clone(),
            txid: key,
            vout: None,
            amount: units_to_decimal(head, asset.decimals),
            confirmations: if solid >= head { self.confirmations_required.max(1) } else { 0 },
            block_height: None,
        }])
    }

    // ---- sweeps ------------------------------------------------------------

    async fn sweep_trx(&self, signer: &SigningKey, to: &[u8; 21], req: &SweepRequest) -> Result<SweepResult> {
        let from = address_of(signer);
        let (balance, _) = self.account(&from, true).await?;
        let wanted = units_from_decimal(req.amount, 6).min(balance as u128) as u64;
        // Keep what the transfer burns: the account creation fee when the settlement
        // account does not exist yet (it replaces the bandwidth), else the bandwidth when
        // the deposit has no free bandwidth left.
        let target_exists = self.account(to, false).await?.1;
        let short_of_bandwidth = self.resources(&from).await?.0 < TRX_TX_BYTES;
        let reserve = match (target_exists, short_of_bandwidth) {
            (false, _) => self.params().await?.create_account_fee,
            (true, true) => TRX_TX_BYTES * self.params().await?.byte_fee,
            (true, false) => 0,
        };
        let send = wanted.min(balance.saturating_sub(reserve));
        if send == 0 {
            return Err(Error::Driver("tron: nothing to sweep after fees".into()));
        }
        let txid = self.send_trx(signer, to, send).await?;
        let requested = units_from_decimal(req.amount, 6) as u64;
        Ok(SweepResult { txid, network_fee: sun_to_trx(requested.saturating_sub(send)) })
    }

    async fn sweep_trc20(
        &self,
        w: &WalletContext,
        signer: &SigningKey,
        to: &[u8; 21],
        contract: &[u8; 21],
        req: &SweepRequest,
        asset: &AssetContext,
    ) -> Result<SweepResult> {
        let hot = hot_key(w)?;
        let from = address_of(signer);
        let balance = self.trc20_balance(contract, &from, false).await?;
        let amount = units_from_decimal(req.amount, asset.decimals).min(balance);
        if amount == 0 {
            return Err(Error::Driver("tron: nothing to sweep".into()));
        }
        let data = transfer_call(to, amount);
        let parameter = hex::encode(&data[4..]);
        let energy = self.estimate_transfer_energy(&from, contract, &parameter).await?;
        let p = self.params().await?;
        let (bandwidth, own_energy) = self.resources(&from).await?;
        let (trx, existed) = self.account(&from, false).await?;
        // 5 % above the estimate: the energy price and the contract's dynamic factor
        // can change between the estimate and the transfer.
        let energy_sun = energy.saturating_sub(own_energy) * p.energy_fee * 105 / 100;
        // A new account gets free bandwidth when the fee transfer activates it.
        let bandwidth_sun = if existed && bandwidth < TRC20_TX_BYTES { TRC20_TX_BYTES * p.byte_fee } else { 0 };
        let need = energy_sun + bandwidth_sun;
        let mut sent = 0;
        let mut funding = Vec::new();
        if trx < need {
            sent = need - trx;
            funding.push(self.send_trx(&hot, &from, sent).await?);
            self.wait_balance(&from, need, 60).await?;
        }
        if !existed && self.resources(&from).await?.0 < TRC20_TX_BYTES {
            let extra = TRC20_TX_BYTES * p.byte_fee;
            sent += extra;
            funding.push(self.send_trx(&hot, &from, extra).await?);
            self.wait_balance(&from, need + extra, 60).await?;
        }
        let fee_limit = (energy * p.energy_fee * 3 / 2).max(SUN_PER_TRX);
        let txid = self.trigger(signer, contract, &data, fee_limit).await?;
        // The transfer is out: from here on only a failed receipt fails the sweep.
        let mut burned = 0;
        match self.wait_info(&txid, 60).await {
            Some(info) => {
                let result = info.pointer("/receipt/result").and_then(Value::as_str).unwrap_or("SUCCESS");
                if result != "SUCCESS" || info.get("result").and_then(Value::as_str) == Some("FAILED") {
                    return Err(Error::Driver(format!("tron: token transfer {txid} failed: {result}")));
                }
                burned = info.get("fee").and_then(Value::as_u64).unwrap_or(0);
            }
            None => tracing::warn!(%txid, "tron token sweep not in a block within 60 s"),
        }
        let mut funding_fees = 0;
        for f in &funding {
            if let Some(info) = self.wait_info(f, 10).await {
                funding_fees += info.get("fee").and_then(Value::as_u64).unwrap_or(0);
            }
        }
        tracing::info!(
            %txid,
            funding = ?funding,
            trx_cost = %sun_to_trx(sent + funding_fees),
            trx_burned = %sun_to_trx(burned),
            energy,
            "tron token sweep: TRX from the hot wallet"
        );
        Ok(SweepResult { txid, network_fee: Decimal::ZERO })
    }

    // ---- tools (tests, funding) -------------------------------------------

    /// Send TRX from a key (32-byte secp256k1 secret).
    pub async fn send_trx_from_key(&self, secret: &[u8], to: &str, sun: u64) -> Result<String> {
        self.send_trx(&key_from_secret(secret)?, &parse_address(to)?, sun).await
    }

    /// Send TRC-20 units from a key.
    pub async fn send_token_from_key(&self, secret: &[u8], contract: &str, to: &str, units: u128) -> Result<String> {
        let data = transfer_call(&parse_address(to)?, units);
        self.trigger(&key_from_secret(secret)?, &parse_address(contract)?, &data, 100 * SUN_PER_TRX).await
    }

    /// Deploy a contract from a key, with its JSON ABI (indexers need it to read
    /// events). Returns the contract address once it is in a block.
    pub async fn deploy_contract(&self, secret: &[u8], name: &str, bytecode: &[u8], abi_json: &str) -> Result<String> {
        let signer = key_from_secret(secret)?;
        let owner = address_of(&signer);
        let abi: Value = serde_json::from_str(abi_json).map_err(|e| Error::BadRequest(format!("abi: {e}")))?;
        let mut contract = Vec::new();
        pb_bytes(&mut contract, 1, &owner);
        pb_bytes(&mut contract, 3, &abi_message(&abi));
        pb_bytes(&mut contract, 4, bytecode);
        pb_uint(&mut contract, 6, 100);
        pb_bytes(&mut contract, 7, name.as_bytes());
        pb_uint(&mut contract, 8, 10_000_000);
        let mut param = Vec::new();
        pb_bytes(&mut param, 1, &owner);
        pb_bytes(&mut param, 2, &contract);
        let raw = self.make_raw(CREATE_SMART_CONTRACT, "CreateSmartContract", &param, 1000 * SUN_PER_TRX).await?;
        let txid = self.broadcast(&signer, &raw).await?;
        let info = self.wait_info(&txid, 60).await.ok_or_else(|| Error::Driver(format!("tron deploy {txid}: not in a block")))?;
        let address = info.get("contract_address").and_then(Value::as_str).unwrap_or_default();
        parse_address(address).map(|a| to_b58(&a))
    }

    /// TRC-20 balance (head state) in token units.
    pub async fn token_balance(&self, contract: &str, owner: &str) -> Result<u128> {
        self.trc20_balance(&parse_address(contract)?, &parse_address(owner)?, false).await
    }

    /// TRX balance (head state) in sun.
    pub async fn balance_sun(&self, address: &str) -> Result<u64> {
        Ok(self.account(&parse_address(address)?, false).await?.0)
    }
}

/// Tron address (base58) of a 32-byte secp256k1 secret.
pub fn address_from_secret(secret: &[u8]) -> Result<String> {
    Ok(to_b58(&address_of(&key_from_secret(secret)?)))
}

fn key_from_secret(secret: &[u8]) -> Result<SigningKey> {
    SigningKey::from_slice(secret).map_err(|e| Error::Driver(format!("tron key: {e}")))
}

fn hot_key(w: &WalletContext) -> Result<SigningKey> {
    let secret = w
        .hot_secret
        .as_ref()
        .ok_or_else(|| Error::Driver("tron: a TRC-20 sweep needs a Tron hot wallet to pay the fees. Assign one to the project.".into()))?;
    key_from_secret(secret)
}

fn signing_key(seed: &str, index: u64) -> SigningKey {
    let mut h = <Sha256 as Digest>::new();
    h.update(seed.as_bytes());
    h.update(b"/tron/");
    h.update(index.to_le_bytes());
    let digest = h.finalize();
    SigningKey::from_slice(&digest).unwrap_or_else(|_| {
        // Digest >= curve order (probability ~2^-128): derive again from it.
        let mut h2 = <Sha256 as Digest>::new();
        h2.update(digest);
        h2.update(b"x");
        SigningKey::from_slice(&h2.finalize()).expect("valid scalar")
    })
}

fn address_of(key: &SigningKey) -> [u8; 21] {
    let point = key.verifying_key().to_sec1_point(false);
    let hash = Keccak256::digest(&point.as_bytes()[1..]);
    let mut addr = [0u8; 21];
    addr[0] = 0x41;
    addr[1..].copy_from_slice(&hash[12..32]);
    addr
}

fn to_b58(addr: &[u8; 21]) -> String {
    let checksum = Sha256::digest(Sha256::digest(addr));
    let mut full = addr.to_vec();
    full.extend_from_slice(&checksum[..4]);
    bs58::encode(full).into_string()
}

/// A base58check address (`T...`) or hex with the 0x41 prefix.
fn parse_address(s: &str) -> Result<[u8; 21]> {
    let s = s.trim();
    let bad = || Error::Driver(format!("bad tron address {s}"));
    if s.len() == 42 && s.starts_with("41") {
        let v = hex::decode(s).map_err(|_| bad())?;
        return v.try_into().map_err(|_| bad());
    }
    let v = bs58::decode(s).into_vec().map_err(|_| bad())?;
    if v.len() != 25 || v[0] != 0x41 {
        return Err(bad());
    }
    let (payload, checksum) = v.split_at(21);
    if &Sha256::digest(Sha256::digest(payload))[..4] != checksum {
        return Err(bad());
    }
    payload.try_into().map_err(|_| bad())
}

fn same_address(s: &str, addr: &[u8; 21]) -> bool {
    parse_address(s).is_ok_and(|a| &a == addr)
}

/// Mainnet mode accepts only a mainnet node. Testnet mode accepts Nile, Shasta and an
/// unknown chain (a local node has its own genesis), but not mainnet.
fn check_genesis(id: &str, testnet: bool) -> Result<()> {
    let network = match id {
        MAINNET_GENESIS => "mainnet",
        NILE_GENESIS => "Nile",
        SHASTA_GENESIS => "Shasta",
        _ => "an unknown network",
    };
    match (testnet, id == MAINNET_GENESIS) {
        (false, true) | (true, false) => Ok(()),
        (false, false) => Err(Error::Driver(format!("tron: this node is on {network}, not on mainnet (genesis block {id})"))),
        (true, true) => Err(Error::Driver("tron: this node is on mainnet, but this install is in testnet mode".into())),
    }
}

fn block_number(b: &Value) -> Result<i64> {
    b.pointer("/block_header/raw_data/number").and_then(Value::as_i64).ok_or_else(|| Error::Driver(format!("tron: no block number in {b}")))
}

/// java-tron sends error messages hex-encoded.
fn decode_message(m: &str) -> String {
    hex::decode(m).ok().and_then(|b| String::from_utf8(b).ok()).unwrap_or_else(|| m.to_string())
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn abi_address(addr: &[u8; 21]) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[12..].copy_from_slice(&addr[1..]);
    word
}

/// Calldata of `transfer(to, amount)`.
fn transfer_call(to: &[u8; 21], amount: u128) -> Vec<u8> {
    let mut data = Keccak256::digest(TRANSFER_SELECTOR.as_bytes())[..4].to_vec();
    data.extend_from_slice(&abi_address(to));
    data.extend_from_slice(&[0u8; 16]);
    data.extend_from_slice(&amount.to_be_bytes());
    data
}

// ---- protobuf (only what transactions need) ---------------------------------

/// `protocol.SmartContract.ABI` from a JSON ABI.
fn abi_message(abi: &Value) -> Vec<u8> {
    let params = |v: Option<&Value>| -> Vec<Vec<u8>> {
        v.and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|p| {
                let mut m = Vec::new();
                pb_uint(&mut m, 1, p.get("indexed").and_then(Value::as_bool).unwrap_or(false) as u64);
                pb_bytes(&mut m, 2, p.get("name").and_then(Value::as_str).unwrap_or_default().as_bytes());
                pb_bytes(&mut m, 3, p.get("type").and_then(Value::as_str).unwrap_or_default().as_bytes());
                m
            })
            .collect()
    };
    let mut out = Vec::new();
    for e in abi.as_array().into_iter().flatten() {
        let kind = match e.get("type").and_then(Value::as_str).unwrap_or("function") {
            "constructor" => 1,
            "function" => 2,
            "event" => 3,
            "fallback" => 4,
            "receive" => 5,
            "error" => 6,
            _ => 0,
        };
        let mutability = match e.get("stateMutability").and_then(Value::as_str).unwrap_or_default() {
            "pure" => 1,
            "view" => 2,
            "nonpayable" => 3,
            "payable" => 4,
            _ => 0,
        };
        let mut m = Vec::new();
        pb_uint(&mut m, 1, e.get("anonymous").and_then(Value::as_bool).unwrap_or(false) as u64);
        pb_uint(&mut m, 2, matches!(mutability, 1 | 2) as u64);
        pb_bytes(&mut m, 3, e.get("name").and_then(Value::as_str).unwrap_or_default().as_bytes());
        for p in params(e.get("inputs")) {
            pb_bytes(&mut m, 4, &p);
        }
        for p in params(e.get("outputs")) {
            pb_bytes(&mut m, 5, &p);
        }
        pb_uint(&mut m, 6, kind);
        pb_uint(&mut m, 7, (mutability == 4) as u64);
        pb_uint(&mut m, 8, mutability);
        pb_bytes(&mut out, 1, &m);
    }
    out
}

fn pb_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// A varint field. Zero is the proto3 default and is left out.
fn pb_uint(out: &mut Vec<u8>, field: u64, v: u64) {
    if v != 0 {
        pb_varint(out, field << 3);
        pb_varint(out, v);
    }
}

fn pb_bytes(out: &mut Vec<u8>, field: u64, b: &[u8]) {
    if !b.is_empty() {
        pb_varint(out, (field << 3) | 2);
        pb_varint(out, b.len() as u64);
        out.extend_from_slice(b);
    }
}

/// Expiration of a transaction made at `now`. The node checks it against its head block
/// time, which lags on an idle local chain: never before the head block, never more
/// than a day after it.
fn expiration(r: &RefBlock, now: i64) -> i64 {
    (now.max(r.timestamp) + EXPIRATION_MS).min(r.timestamp + 23 * 3_600_000)
}

/// `protocol.Transaction.raw` with one contract, fields in number order (as the node
/// serializes them to compute the txid).
fn raw_transaction(r: &RefBlock, expiration: i64, now: i64, contract_type: u64, type_name: &str, param: &[u8], fee_limit: u64) -> Vec<u8> {
    let mut any = Vec::new();
    pb_bytes(&mut any, 1, format!("type.googleapis.com/protocol.{type_name}").as_bytes());
    pb_bytes(&mut any, 2, param);
    let mut contract = Vec::new();
    pb_uint(&mut contract, 1, contract_type);
    pb_bytes(&mut contract, 2, &any);
    let mut raw = Vec::new();
    pb_bytes(&mut raw, 1, &r.bytes);
    pb_bytes(&mut raw, 4, &r.hash);
    pb_uint(&mut raw, 8, expiration as u64);
    pb_bytes(&mut raw, 11, &contract);
    pb_uint(&mut raw, 14, now as u64);
    pb_uint(&mut raw, 18, fee_limit);
    raw
}

fn sun_to_trx(s: u64) -> Decimal {
    Decimal::from(s) / Decimal::from(SUN_PER_TRX)
}

fn units_to_decimal(units: u128, decimals: u32) -> Decimal {
    Decimal::from_i128_with_scale(units as i128, decimals).normalize()
}

fn units_from_decimal(d: Decimal, decimals: u32) -> u128 {
    (d * Decimal::from(10u64.pow(decimals))).floor().to_u128().unwrap_or(0)
}

#[async_trait]
impl PaymentDriver for TronDriver {
    fn chain(&self) -> &str {
        "tron"
    }

    fn can_derive(&self) -> bool {
        self.master_seed.is_some()
    }

    async fn validate_address(&self, address: &str) -> Result<bool> {
        Ok(address.len() == 34 && parse_address(address).is_ok())
    }

    async fn derive_deposit(&self, _w: &WalletContext, derivation_index: u64) -> Result<DerivedAddress> {
        Ok(DerivedAddress {
            address: to_b58(&address_of(&signing_key(self.seed()?, derivation_index))),
            payin_extra_id: None,
            derivation_index,
        })
    }

    async fn observe(
        &self,
        _w: &WalletContext,
        address: &str,
        _payin_extra_id: Option<&str>,
        asset: &AssetContext,
    ) -> Result<Vec<Observation>> {
        if self.indexed {
            let (latest, solid) = (self.latest_block().await?, self.solid_block().await?);
            return self.observe_indexed(address, asset, latest, solid).await;
        }
        self.observe_balance(address, asset).await
    }

    async fn sweep(&self, w: &WalletContext, req: &SweepRequest, asset: &AssetContext) -> Result<SweepResult> {
        let signer = signing_key(self.seed()?, req.derivation_index);
        if to_b58(&address_of(&signer)) != req.from_address {
            return Err(Error::Driver(format!("tron: index {} does not derive {}", req.derivation_index, req.from_address)));
        }
        let to = parse_address(&req.to_address)?;
        match asset.contract_address.as_deref() {
            None => self.sweep_trx(&signer, &to, req).await,
            Some(contract) => self.sweep_trc20(w, &signer, &to, &parse_address(contract)?, req, asset).await,
        }
    }

    /// Final once its block is solid. A failed receipt fails it; so does no block after its expiration.
    async fn sweep_state(&self, sweep: &SweepCheck, _asset: &AssetContext) -> Result<SweepState> {
        let solid = self.solid_block().await?;
        Ok(match self.tx_state(&sweep.txid, solid).await? {
            TxState::Failed => SweepState::Failed("the transaction failed on chain".into()),
            TxState::InBlock(block) if block <= solid => SweepState::Confirmed,
            TxState::InBlock(_) => SweepState::Pending,
            TxState::Pending if sweep.expired() => SweepState::Failed("the transaction is in no block and has expired".into()),
            TxState::Pending => SweepState::Pending,
        })
    }

    async fn tip_height(&self) -> Result<i64> {
        self.latest_block().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn driver(seed: Option<&str>) -> TronDriver {
        TronDriver {
            client: reqwest::Client::new(),
            base_url: String::new(),
            api_key: None,
            master_seed: seed.map(|s| Zeroizing::new(s.to_string())),
            confirmations_required: 19,
            indexed: false,
            blocks: Mutex::new(HashMap::new()),
        }
    }

    #[tokio::test]
    async fn derives_valid_tron_addresses() {
        let d = driver(Some("unit-test-seed"));
        let w = WalletContext::default();
        let a0 = d.derive_deposit(&w, 0).await.unwrap().address;
        let a1 = d.derive_deposit(&w, 1).await.unwrap().address;
        assert!(a0.starts_with('T') && a0.len() == 34, "{a0}");
        assert_ne!(a0, a1);
        assert!(d.validate_address(&a0).await.unwrap());
        assert!(!d.validate_address("Tnotarealaddressxxxxxxxxxxxxxxxxxx").await.unwrap());
        assert_eq!(a0, d.derive_deposit(&w, 0).await.unwrap().address);
        // Stable derivation: payments already made depend on it.
        assert_eq!(a0, to_b58(&address_of(&signing_key("unit-test-seed", 0))));
    }

    #[tokio::test]
    async fn without_seed_it_cannot_derive_or_sweep() {
        let d = driver(None);
        assert!(!d.can_derive());
        assert!(d.derive_deposit(&WalletContext::default(), 1).await.is_err());
        assert!(d.validate_address(BLACK_HOLE).await.unwrap());
    }

    #[test]
    fn encodes_a_transfer_like_the_node() {
        // A TransferContract made by a java-tron node (`/wallet/createtransaction`).
        let r = RefBlock { bytes: [0, 1], hash: hex::decode("e4e2e3e301e26f96").unwrap().try_into().unwrap(), timestamp: 0 };
        let mut param = Vec::new();
        pb_bytes(&mut param, 1, &parse_address("TBMspfKq6FS8hXjXbxmFrpiN8xXmreBSmP").unwrap());
        pb_bytes(&mut param, 2, &parse_address("TMUYArnM9d8kmhi5YykepgDhthFi5XhHji").unwrap());
        pb_uint(&mut param, 3, 1234567);
        let raw = raw_transaction(&r, 1792822427269, 1792819960747, TRANSFER_CONTRACT, "TransferContract", &param, 0);
        assert_eq!(
            hex::encode(&raw),
            "0a0200012208e4e2e3e301e26f964085fdace496345a67080112630a2d747970652e676f6f676c65617069732e636f6d2f70726f746f636f6c2e5472616e73666572436f6e747261637412320a15410f41aecfec37cca4a9a144b535fec4af677bec8f1215417e3572ada05d28f1b795f59a57f15be97d3c7f7c1887ad4b70abb796e39634"
        );
        assert_eq!(hex::encode(Sha256::digest(&raw)), "b9c87a98985accc47ba5f7a4cc89ee49bdd7035dbd851c69374ad45456422ec3");
    }

    #[test]
    fn expiration_follows_the_head_block() {
        let r = RefBlock { bytes: [0, 1], hash: [0; 8], timestamp: 1_000_000 };
        assert_eq!(expiration(&r, 1_000_500), 1_000_500 + EXPIRATION_MS);
        assert_eq!(expiration(&r, 10), 1_000_000 + EXPIRATION_MS);
        assert_eq!(expiration(&r, 1_000_000 + 48 * 3_600_000), 1_000_000 + 23 * 3_600_000);
    }

    #[test]
    fn confirmations_wait_for_solidification() {
        let d = driver(None);
        assert_eq!(d.confirmations(100, 110, 99), 11);
        assert_eq!(d.confirmations(100, 130, 110), 31);
        let mut d1 = driver(None);
        d1.confirmations_required = 1;
        assert_eq!(d1.confirmations(100, 100, 99), 0);
        assert_eq!(d1.confirmations(100, 100, 100), 1);
    }

    #[test]
    fn genesis_must_match_the_mode() {
        let local = "0000000000000000c93baa76a4a508f798a96f59156d9eb17ecede8ec845df2f";
        assert!(check_genesis(MAINNET_GENESIS, false).is_ok());
        for other in [NILE_GENESIS, SHASTA_GENESIS, local, ""] {
            assert!(check_genesis(other, false).is_err(), "{other} on mainnet");
            assert!(check_genesis(other, true).is_ok(), "{other} on testnet");
        }
        assert!(check_genesis(MAINNET_GENESIS, true).is_err());
    }

    #[test]
    fn amounts_and_calldata() {
        assert_eq!(units_from_decimal(Decimal::new(12345, 3), 6), 12_345_000);
        assert_eq!(units_to_decimal(12_345_000, 6), Decimal::new(12345, 3));
        let to = parse_address("TMUYArnM9d8kmhi5YykepgDhthFi5XhHji").unwrap();
        let data = transfer_call(&to, 5);
        assert_eq!(hex::encode(&data[..4]), "a9059cbb");
        assert_eq!(data.len(), 68);
        assert_eq!(data[67], 5);
    }
}
