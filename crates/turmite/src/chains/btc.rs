//! Bitcoin driver over bitcoind JSON-RPC. Watch-only: the node holds no private key.
//!
//! Each project has a watch-only descriptor wallet `turmite-p<project_id>` with the
//! receive chain of the merchant's account key, `wpkh(<xpub>/0/*)` (BIP84). A payment's
//! deposit address is the receive address at the next index, so the coins land in the
//! merchant's own wallet and nothing is swept. Each deposit address has itself as its
//! label, so `observe` lists exactly its transactions.
//!
//! A deposit that leaves the mempool without a confirmation (replaced or evicted) is
//! reported with `confirmations = -1`; the store then stops counting it.

use std::{collections::HashMap, time::Duration};

use async_trait::async_trait;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    chains::driver::{AssetContext, DerivedAddress, IndexScope, Observation, PaymentDriver, SweepRequest, SweepResult, WalletContext},
    error::{Error, Result},
};

const RPC_INVALID_ADDRESS_OR_KEY: i64 = -5;
const RPC_WALLET_NOT_FOUND: i64 = -18;
const RPC_WALLET_ALREADY_LOADED: i64 = -35;

/// Receive addresses the node watches past the deposit index it hands out.
const LOOKAHEAD: u64 = 1000;
/// Extend the watched range when a new index comes this close to its end.
const MARGIN: u64 = 100;

/// Confirmations reported for a deposit that left the mempool without a block.
pub const DROPPED: i32 = -1;

const VERSION_XPUB: u32 = 0x0488_b21e;
const VERSION_TPUB: u32 = 0x0435_87cf;

/// Name of a project's watch-only wallet on the node.
pub fn wallet_name(project_id: i64) -> String {
    format!("turmite-p{project_id}")
}

pub struct BtcDriver {
    client: reqwest::Client,
    base_url: String,
    user: Option<String>,
    password: Option<String>,
    /// Project wallets that are loaded and watch the xpub, with the end of the watched range.
    ready: std::sync::Mutex<HashMap<i64, (String, u64)>>,
    /// One wallet setup at a time.
    setup: tokio::sync::Mutex<()>,
    unconfirmed: Unconfirmed,
}

struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn into_error(self, method: &str) -> Error {
        Error::Driver(format!("bitcoind {method}: {} (code {})", self.message, self.code))
    }
}

impl BtcDriver {
    /// Connect to bitcoind and load the project wallets it has on disk.
    pub async fn connect(base_url: &str, user: Option<String>, password: Option<String>) -> Result<Self> {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(120)).build().map_err(|e| Error::Internal(e.to_string()))?;
        let driver = Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            user,
            password,
            ready: Default::default(),
            setup: Default::default(),
            unconfirmed: Default::default(),
        };
        driver.node("getblockchaininfo", json!([])).await?;
        if let Ok(dir) = driver.node("listwalletdir", json!([])).await {
            let names = dir["wallets"].as_array().into_iter().flatten().filter_map(|w| w["name"].as_str());
            for name in names.filter(|n| n.starts_with("turmite-p")) {
                if let Err(e) = driver.load(name).await {
                    tracing::warn!(wallet = name, error = %e, "cannot load a bitcoin watch-only wallet");
                }
            }
        }
        Ok(driver)
    }

    /// One JSON-RPC call. The outer error is a transport failure, the inner one a node error.
    async fn call(&self, wallet: Option<&str>, method: &str, params: Value) -> Result<std::result::Result<Value, RpcError>> {
        let endpoint = match wallet {
            Some(w) => format!("{}/wallet/{w}", self.base_url),
            None => self.base_url.clone(),
        };
        let mut req = self.client.post(&endpoint).json(&json!({ "jsonrpc": "2.0", "id": "turmite", "method": method, "params": params }));
        if let Some(u) = &self.user {
            req = req.basic_auth(u, self.password.as_deref());
        }
        let resp = req.send().await.map_err(|e| Error::Driver(format!("bitcoind {method}: {}", e.without_url())))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(Error::Driver("bitcoind refused the RPC user or password".into()));
        }
        let body: Value = resp.json().await.map_err(|e| Error::Driver(format!("bitcoind {method}: HTTP {status}: {}", e.without_url())))?;
        if let Some(err) = body.get("error").filter(|e| !e.is_null()) {
            return Ok(Err(RpcError {
                code: err["code"].as_i64().unwrap_or(0),
                message: err["message"].as_str().unwrap_or_default().to_string(),
            }));
        }
        Ok(Ok(body.get("result").cloned().unwrap_or(Value::Null)))
    }

    async fn node(&self, method: &str, params: Value) -> Result<Value> {
        self.call(None, method, params).await?.map_err(|e| e.into_error(method))
    }

    /// A call on a project's wallet. Loads the wallet again when the node lost it (restart).
    async fn wallet(&self, project_id: i64, method: &str, params: Value) -> Result<Value> {
        let name = wallet_name(project_id);
        match self.call(Some(&name), method, params.clone()).await? {
            Ok(v) => Ok(v),
            Err(e) if e.code == RPC_WALLET_NOT_FOUND => {
                if !self.load(&name).await? {
                    self.ready.lock().unwrap_or_else(|e| e.into_inner()).remove(&project_id);
                    return Err(Error::Driver(format!("bitcoind: wallet {name} is missing")));
                }
                self.call(Some(&name), method, params).await?.map_err(|e| e.into_error(method))
            }
            Err(e) => Err(e.into_error(method)),
        }
    }

    /// Load a wallet (and keep loading it when the node starts). False when it does not exist.
    async fn load(&self, name: &str) -> Result<bool> {
        match self.call(None, "loadwallet", json!([name, true])).await? {
            Ok(_) => Ok(true),
            Err(e) if e.code == RPC_WALLET_ALREADY_LOADED => Ok(true),
            Err(e) if e.code == RPC_WALLET_NOT_FOUND => Ok(false),
            Err(e) => Err(e.into_error("loadwallet")),
        }
    }

    /// Load a watch-only wallet, making it when it does not exist. Retries while another
    /// caller on the same node holds the wallet file.
    async fn open_or_create(&self, name: &str) -> Result<()> {
        let mut last = Error::Driver(format!("bitcoind: cannot open wallet {name}"));
        for _ in 0..20 {
            match self.load(name).await {
                Ok(true) => return Ok(()),
                Ok(false) => {
                    let params = json!({
                        "wallet_name": name,
                        "disable_private_keys": true,
                        "blank": true,
                        "descriptors": true,
                        "load_on_startup": true,
                    });
                    match self.call(None, "createwallet", params).await? {
                        Ok(_) => return Ok(()),
                        Err(e) => last = e.into_error("createwallet"),
                    }
                }
                Err(e) => last = e,
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(last)
    }

    /// Make sure the project's wallet exists, is loaded and watches the receive chain of its
    /// xpub up to at least `index + MARGIN`. Returns the receive descriptor with its checksum.
    async fn ensure(&self, w: &WalletContext, index: u64) -> Result<String> {
        let xpub = w
            .xpub
            .as_deref()
            .ok_or_else(|| Error::BadRequest("This project has no Bitcoin account key. Add it on the Wallets page.".into()))?;
        let desc = receive_descriptor(xpub);
        let is_ready = || {
            let ready = self.ready.lock().unwrap_or_else(|e| e.into_inner());
            ready.get(&w.project_id).is_some_and(|(x, end)| x == xpub && *end >= index + MARGIN)
        };
        if is_ready() {
            return Ok(desc);
        }
        let _setup = self.setup.lock().await;
        if is_ready() {
            return Ok(desc);
        }
        self.open_or_create(&wallet_name(w.project_id)).await?;
        // Another process on the same node can import at the same moment: read and try again.
        let mut attempt = 0;
        let end = loop {
            let current = self.watched_range_end(w.project_id, &desc).await?;
            if let Some(end) = current.filter(|end| *end >= index + MARGIN) {
                break end;
            }
            let end = (index + LOOKAHEAD).max(current.unwrap_or(0));
            let timestamp = import_timestamp(current.is_some(), w.created_at);
            let import = json!([[{ "desc": desc, "active": true, "range": [0, end], "timestamp": timestamp, "internal": false }]]);
            let error = match self.wallet(w.project_id, "importdescriptors", import).await {
                Ok(res) if res[0]["success"].as_bool() == Some(true) => {
                    // The node may watch more than asked (keypool).
                    break self.watched_range_end(w.project_id, &desc).await?.unwrap_or(end);
                }
                // A failed rescan (for example on a pruned node) keeps the descriptor: the next
                // read finds it, but deposits in the blocks the node could not read are missing.
                Ok(res) => {
                    tracing::warn!(project_id = w.project_id, error = %res[0]["error"], "bitcoind import of the watch-only wallet failed");
                    Error::Driver(format!("bitcoind importdescriptors: {}", res[0]["error"]))
                }
                // For example "Wallet is currently rescanning" during another import.
                Err(e) => e,
            };
            attempt += 1;
            if attempt == 20 {
                return Err(error);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        };
        self.ready.lock().unwrap_or_else(|e| e.into_inner()).insert(w.project_id, (xpub.to_string(), end));
        Ok(desc)
    }

    /// End of the range the wallet watches for `desc`, if it has the descriptor.
    async fn watched_range_end(&self, project_id: i64, desc: &str) -> Result<Option<u64>> {
        let list = self.wallet(project_id, "listdescriptors", json!([])).await?;
        let body = desc.split('#').next().unwrap_or(desc);
        Ok(list["descriptors"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|d| d["desc"].as_str().and_then(|s| s.split('#').next()) == Some(body))
            .and_then(|d| d["range"][1].as_u64()))
    }

    /// Confirmations of an output the wallet lists as unconfirmed: 0 while in the mempool,
    /// the chain's count when a block has it (the wallet lags), else `DROPPED`.
    async fn unconfirmed_state(&self, txid: &str, vout: i32) -> Result<i32> {
        match self.call(None, "getmempoolentry", json!([txid])).await? {
            Ok(_) => Ok(0),
            Err(e) if e.code == RPC_INVALID_ADDRESS_OR_KEY => {
                let out = self.node("gettxout", json!([txid, vout, true])).await?;
                Ok(if out.is_null() { DROPPED } else { out["confirmations"].as_i64().unwrap_or(0).clamp(0, i32::MAX as i64) as i32 })
            }
            Err(e) => Err(e.into_error("getmempoolentry")),
        }
    }

    /// State of an earlier unconfirmed deposit that the wallet no longer lists.
    async fn missing_state(&self, project_id: i64, txid: &str, vout: i32) -> Result<i32> {
        let name = wallet_name(project_id);
        match self.call(Some(&name), "gettransaction", json!([txid, true])).await? {
            Ok(tx) => match tx["confirmations"].as_i64().unwrap_or(0) {
                c if c < 0 => Ok(DROPPED),
                0 => self.unconfirmed_state(txid, vout).await,
                c => Ok(c.min(i32::MAX as i64) as i32),
            },
            Err(e) if e.code == RPC_INVALID_ADDRESS_OR_KEY => Ok(DROPPED),
            Err(e) => Err(e.into_error("gettransaction")),
        }
    }
}

#[async_trait]
impl PaymentDriver for BtcDriver {
    fn chain(&self) -> &str {
        "bitcoin"
    }

    fn index_scope(&self) -> IndexScope {
        IndexScope::Wallet
    }

    fn needs_sweep(&self) -> bool {
        false
    }

    async fn validate_address(&self, address: &str) -> Result<bool> {
        let res = self.node("validateaddress", json!([address])).await?;
        Ok(res["isvalid"].as_bool().unwrap_or(false))
    }

    /// Check the xpub against the node's network, set up the watch-only wallet, and return
    /// the first three receive addresses for the operator to compare with the wallet's.
    async fn prepare_wallet(&self, w: &WalletContext) -> Result<Vec<String>> {
        let xpub = w.xpub.as_deref().ok_or_else(|| Error::BadRequest("Enter the account key (xpub or zpub).".into()))?;
        let chain = self.node("getblockchaininfo", json!([])).await?["chain"].as_str().unwrap_or_default().to_string();
        let mainnet_key = xpub.starts_with("xpub");
        if mainnet_key != (chain == "main") {
            let want = if chain == "main" { "xpub or zpub" } else { "tpub or vpub" };
            return Err(Error::BadRequest(format!("The Bitcoin node runs on {chain}. Use a {want} key for this network.")));
        }
        let desc = self.ensure(w, 0).await?;
        let res = self.node("deriveaddresses", json!([desc, [0, 2]])).await?;
        Ok(res.as_array().into_iter().flatten().filter_map(|a| a.as_str().map(str::to_string)).collect())
    }

    async fn derive_deposit(&self, w: &WalletContext, derivation_index: u64) -> Result<DerivedAddress> {
        let desc = self.ensure(w, derivation_index).await?;
        let res = self.node("deriveaddresses", json!([desc, [derivation_index, derivation_index]])).await?;
        let address = res[0].as_str().ok_or_else(|| Error::Driver("deriveaddresses: no address".into()))?.to_string();
        self.wallet(w.project_id, "setlabel", json!([address, address])).await?;
        Ok(DerivedAddress { address, payin_extra_id: None, derivation_index })
    }

    async fn observe(
        &self,
        w: &WalletContext,
        address: &str,
        _payin_extra_id: Option<&str>,
        asset: &AssetContext,
    ) -> Result<Vec<Observation>> {
        if !asset.is_native() {
            return Err(Error::Driver("bitcoin: token assets are not supported".into()));
        }
        self.ensure(w, 0).await?;
        let entries = self.wallet(w.project_id, "listtransactions", json!([address, 1000, 0, true])).await?;
        let mut out = Vec::new();
        for e in entries.as_array().into_iter().flatten() {
            if e["category"].as_str() != Some("receive") || e["address"].as_str() != Some(address) {
                continue;
            }
            let (Some(txid), Some(vout)) = (e["txid"].as_str(), e["vout"].as_i64()) else { continue };
            let vout = vout as i32;
            let confirmations = match e["confirmations"].as_i64().unwrap_or(0) {
                c if c < 0 => DROPPED,
                0 => self.unconfirmed_state(txid, vout).await?,
                c => c.min(i32::MAX as i64) as i32,
            };
            out.push(Observation {
                chain_tx_key: format!("{txid}:{vout}"),
                txid: txid.to_string(),
                vout: Some(vout),
                amount: btc_amount(&e["amount"]),
                confirmations,
                block_height: e["blockheight"].as_i64(),
            });
        }
        // A conflicted deposit drops out of the list: ask for the ones seen unconfirmed before.
        for mut o in self.unconfirmed.missing(address, &out) {
            o.confirmations = self.missing_state(w.project_id, &o.txid, o.vout.unwrap_or(0)).await?;
            out.push(o);
        }
        self.unconfirmed.remember(address, &out);
        Ok(out)
    }

    async fn sweep(&self, _w: &WalletContext, _req: &SweepRequest, _asset: &AssetContext) -> Result<SweepResult> {
        Err(Error::Driver("bitcoin deposits land in the merchant's wallet; there is nothing to sweep".into()))
    }

    async fn tip_height(&self) -> Result<i64> {
        self.node("getblockcount", json!([])).await?.as_i64().ok_or_else(|| Error::Driver("getblockcount: no height".into()))
    }
}

/// Unconfirmed deposits seen by earlier `observe` calls, per deposit address. A node can
/// forget a transaction (replaced, evicted, double-spent); the driver then reports it as
/// dropped. Kept in memory: after a restart only new observations are tracked.
#[derive(Default)]
pub(crate) struct Unconfirmed(std::sync::Mutex<HashMap<String, Vec<Observation>>>);

/// Addresses tracked at most; old entries are forgotten past it.
const UNCONFIRMED_MAX: usize = 50_000;

impl Unconfirmed {
    /// Earlier unconfirmed observations of `address` that are not in `seen`.
    pub(crate) fn missing(&self, address: &str, seen: &[Observation]) -> Vec<Observation> {
        let map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.get(address)
            .map(|prev| prev.iter().filter(|p| !seen.iter().any(|o| o.chain_tx_key == p.chain_tx_key)).cloned().collect())
            .unwrap_or_default()
    }

    /// Keep this round's unconfirmed observations of `address`.
    pub(crate) fn remember(&self, address: &str, all: &[Observation]) {
        let pending: Vec<Observation> = all.iter().filter(|o| o.confirmations == 0).cloned().collect();
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if pending.is_empty() {
            map.remove(address);
        } else {
            if map.len() >= UNCONFIRMED_MAX {
                map.clear();
            }
            map.insert(address.to_string(), pending);
        }
    }
}

/// A BTC amount from a JSON number, exact to the satoshi.
fn btc_amount(v: &Value) -> Decimal {
    v.as_f64().and_then(Decimal::from_f64_retain).map(|d| d.round_dp(8).normalize()).unwrap_or(Decimal::ZERO)
}

/// The `timestamp` of a descriptor import: the node rescans the blocks after it.
/// A descriptor that the wallet does not have yet (a new wallet, or another node after a
/// failover) scans from when the project wallet was saved, so the node finds the earlier
/// deposits. A longer range of a known descriptor adds only addresses that Turmite did not
/// hand out yet, so it scans nothing old.
fn import_timestamp(known: bool, wallet_created: Option<chrono::DateTime<chrono::Utc>>) -> Value {
    match wallet_created {
        Some(t) if !known => json!(t.timestamp().max(1)),
        _ => json!("now"),
    }
}

/// `wpkh(<xpub>/0/*)#<checksum>`: the receive chain of a BIP84 account.
pub fn receive_descriptor(xpub: &str) -> String {
    let body = format!("wpkh({xpub}/0/*)");
    let checksum = descriptor_checksum(&body).unwrap_or_default();
    format!("{body}#{checksum}")
}

/// BIP380 descriptor checksum. None when the text has a character descriptors do not allow.
pub fn descriptor_checksum(desc: &str) -> Option<String> {
    const INPUT: &str = "0123456789()[],'/*abcdefgh@:$%{}IJKLMNOPQRSTUVWXYZ&+-.;<=>?!^_|~ijklmnopqrstuvwxyzABCDEFGH`#\"\\ ";
    const CHECKSUM: &[u8] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
    fn polymod(c: u64, val: u64) -> u64 {
        let c0 = c >> 35;
        let mut c = ((c & 0x7_ffff_ffff) << 5) ^ val;
        for (bit, k) in [0xf5dee51989u64, 0xa9fdca3312, 0x1bab10e32d, 0x3706b1677a, 0x644d626ffd].into_iter().enumerate() {
            if c0 >> bit & 1 == 1 {
                c ^= k;
            }
        }
        c
    }
    let (mut c, mut cls, mut count) = (1u64, 0u64, 0);
    for ch in desc.chars() {
        let pos = INPUT.find(ch)? as u64;
        c = polymod(c, pos & 31);
        cls = cls * 3 + (pos >> 5);
        count += 1;
        if count == 3 {
            c = polymod(c, cls);
            cls = 0;
            count = 0;
        }
    }
    if count > 0 {
        c = polymod(c, cls);
    }
    for _ in 0..8 {
        c = polymod(c, 0);
    }
    c ^= 1;
    Some((0..8).map(|j| CHECKSUM[((c >> (5 * (7 - j))) & 31) as usize] as char).collect())
}

/// Read an account extended public key and return it in the form bitcoind reads: `xpub…`
/// on mainnet, `tpub…` on the test networks. SLIP-132 `zpub` and `vpub` are converted.
/// The error is a message for the operator.
pub fn parse_xpub(input: &str, mainnet: bool) -> std::result::Result<String, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Enter the account key (xpub or zpub).".into());
    }
    let raw = base58check_decode(input).ok_or("This is not a valid extended public key. Check that you copied all of it.")?;
    if raw.len() != 78 {
        return Err("This is not a valid extended public key. Check that you copied all of it.".into());
    }
    let version = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]);
    let private = [
        0x0488_ade4u32, // xprv
        0x049d_7878,    // yprv
        0x04b2_430c,    // zprv
        0x0295_b005,    // Yprv
        0x02aa_7a99,    // Zprv
        0x0435_8394,    // tprv
        0x044a_4e28,    // uprv
        0x045f_18bc,    // vprv
        0x0242_85b5,    // Uprv
        0x0257_5048,    // Vprv
    ];
    if private.contains(&version) || raw[45] == 0 {
        return Err(
            "This is a private key. Never enter a private key here. Enter the extended public key (xpub or zpub) of the account.".into()
        );
    }
    let is_mainnet = match version {
        VERSION_XPUB | 0x04b2_4746 => true,  // xpub, zpub
        VERSION_TPUB | 0x045f_1cf6 => false, // tpub, vpub
        0x049d_7cb2 | 0x044a_5262 => {
            return Err("This key is for wrapped SegWit (ypub, upub). Use the native SegWit account key (zpub or xpub).".into());
        }
        0x0295_b43f | 0x02aa_7ed3 | 0x0242_89ef | 0x0257_5483 => {
            return Err("Multisig keys are not supported. Use the key of a single-signature native SegWit account.".into());
        }
        _ => return Err("This is not a Bitcoin extended public key.".into()),
    };
    if !matches!(raw[45], 2 | 3) {
        return Err("This is not a valid extended public key.".into());
    }
    if is_mainnet != mainnet {
        return Err(if mainnet {
            "This key is for a test network. This server runs on mainnet: use an xpub or zpub.".into()
        } else {
            "This key is for mainnet. This server runs on a test network: use a tpub or vpub.".into()
        });
    }
    let mut out = raw;
    out[..4].copy_from_slice(&(if mainnet { VERSION_XPUB } else { VERSION_TPUB }).to_be_bytes());
    Ok(base58check_encode(&out))
}

const B58: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

fn sha256d(data: &[u8]) -> [u8; 32] {
    Sha256::digest(Sha256::digest(data)).into()
}

fn base58_decode(s: &str) -> Option<Vec<u8>> {
    let mut bytes: Vec<u8> = Vec::new();
    for c in s.bytes() {
        let mut carry = B58.iter().position(|&b| b == c)? as u32;
        for b in bytes.iter_mut().rev() {
            carry += (*b as u32) * 58;
            *b = carry as u8;
            carry >>= 8;
        }
        while carry > 0 {
            bytes.insert(0, carry as u8);
            carry >>= 8;
        }
    }
    let zeros = s.bytes().take_while(|&c| c == b'1').count();
    let mut out = vec![0u8; zeros];
    out.extend(bytes);
    Some(out)
}

fn base58_encode(data: &[u8]) -> String {
    let mut digits: Vec<u8> = Vec::new();
    for &byte in data {
        let mut carry = byte as u32;
        for d in digits.iter_mut().rev() {
            carry += (*d as u32) << 8;
            *d = (carry % 58) as u8;
            carry /= 58;
        }
        while carry > 0 {
            digits.insert(0, (carry % 58) as u8);
            carry /= 58;
        }
    }
    let zeros = data.iter().take_while(|&&b| b == 0).count();
    std::iter::repeat_n('1', zeros).chain(digits.iter().map(|&d| B58[d as usize] as char)).collect()
}

fn base58check_decode(s: &str) -> Option<Vec<u8>> {
    let raw = base58_decode(s)?;
    if raw.len() < 4 {
        return None;
    }
    let (payload, check) = raw.split_at(raw.len() - 4);
    (sha256d(payload)[..4] == *check).then(|| payload.to_vec())
}

fn base58check_encode(payload: &[u8]) -> String {
    let mut data = payload.to_vec();
    data.extend_from_slice(&sha256d(payload)[..4]);
    base58_encode(&data)
}

#[cfg(test)]
mod tests {
    use super::*;

    // BIP32 test vector 1, chain m/0'.
    const XPUB: &str = "xpub68Gmy5EdvgibQVfPdqkBBCHxA5htiqg55crXYuXoQRKfDBFA1WEjWgP6LHhwBZeNK1VTsfTFUHCdrfp1bgwQ9xv5ski8PX9rL2dZXvgGDnw";
    const XPRV: &str = "xprv9uHRZZhk6KAJC1avXpDAp4MDc3sQKNxDiPvvkX8Br5ngLNv1TxvUxt4cV1rGL5hj6KCesnDYUhd7oWgT11eZG7XnxHrnYeSvkzY7d2bhkJ7";

    fn with_version(key: &str, version: u32) -> String {
        let mut raw = base58check_decode(key).unwrap();
        raw[..4].copy_from_slice(&version.to_be_bytes());
        base58check_encode(&raw)
    }

    #[test]
    fn import_scans_from_the_wallet_creation_on_a_new_node() {
        let created = chrono::DateTime::from_timestamp(1_790_000_000, 0).unwrap();
        assert_eq!(import_timestamp(false, Some(created)), json!(1_790_000_000));
        assert_eq!(import_timestamp(true, Some(created)), json!("now"), "a longer range scans nothing old");
        assert_eq!(import_timestamp(false, None), json!("now"), "a key check before the wallet is saved");
    }

    #[test]
    fn base58_round_trip() {
        let raw = base58check_decode(XPUB).unwrap();
        assert_eq!(raw.len(), 78);
        assert_eq!(base58check_encode(&raw), XPUB);
        assert!(base58check_decode(&XPUB.replace('Q', "R")).is_none(), "checksum must fail");
        assert_eq!(base58_encode(&[0, 0, 1]), "112");
        assert_eq!(base58_decode("112").unwrap(), vec![0, 0, 1]);
    }

    #[test]
    fn slip132_keys_convert() {
        let zpub = with_version(XPUB, 0x04b2_4746);
        assert!(zpub.starts_with("zpub"));
        assert_eq!(parse_xpub(&zpub, true).unwrap(), XPUB);
        assert_eq!(parse_xpub(&format!("  {XPUB}\n"), true).unwrap(), XPUB);

        let tpub = with_version(XPUB, VERSION_TPUB);
        let vpub = with_version(XPUB, 0x045f_1cf6);
        assert!(tpub.starts_with("tpub") && vpub.starts_with("vpub"));
        assert_eq!(parse_xpub(&vpub, false).unwrap(), tpub);
        assert_eq!(parse_xpub(&tpub, false).unwrap(), tpub);
    }

    #[test]
    fn bad_keys_are_refused() {
        assert!(parse_xpub(XPRV, true).unwrap_err().contains("private key"));
        assert!(parse_xpub(&with_version(XPRV, 0x04b2_430c), true).unwrap_err().contains("private key"), "zprv");
        assert!(parse_xpub(&with_version(XPRV, 0x0435_8394), false).unwrap_err().contains("private key"), "tprv");
        assert!(parse_xpub(&with_version(XPUB, 0x049d_7cb2), true).unwrap_err().contains("wrapped SegWit"), "ypub");
        assert!(parse_xpub(&with_version(XPUB, 0x02aa_7ed3), true).unwrap_err().contains("Multisig"), "Zpub");
        assert!(parse_xpub(XPUB, false).unwrap_err().contains("for mainnet"));
        assert!(parse_xpub(&with_version(XPUB, VERSION_TPUB), true).unwrap_err().contains("test network"));
        assert!(parse_xpub("", true).is_err());
        assert!(parse_xpub("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq", true).is_err());
        assert!(parse_xpub(&XPUB[..100], true).is_err());
    }

    #[test]
    fn descriptor_checksum_matches_bitcoind() {
        // Values from `getdescriptorinfo` on bitcoind 31.
        let tpub = "tpubDC6yNtDDCZ6tg3MVdDXGqp5jq7wP1947tLyeC7HYuT4C2g4ccjFMwJut1uuEzSL1r4G62MNtCVaohg9MJgy7Nbq8nrvz8Scb53uFQbGoP2X";
        assert_eq!(receive_descriptor(tpub), format!("wpkh({tpub}/0/*)#hzg8k23h"));
        assert_eq!(descriptor_checksum("addr(mkmZxiEcEd8ZqjQWVZuC6so5dFMKEFpN2j)").as_deref(), Some("02wpgw69"));
        assert_eq!(descriptor_checksum("raw(00)\u{e9}"), None);
    }

    #[test]
    fn amounts_are_exact() {
        assert_eq!(btc_amount(&json!(0.001)), Decimal::new(1, 3));
        assert_eq!(btc_amount(&json!(0.00012345)), Decimal::new(12345, 8));
        assert_eq!(btc_amount(&json!(20999999.99999999)), "20999999.99999999".parse::<Decimal>().unwrap());
        assert_eq!(btc_amount(&json!(1e-8)), Decimal::new(1, 8));
    }

    #[test]
    fn unconfirmed_tracker_reports_missing() {
        let t = Unconfirmed::default();
        let o = |key: &str, c: i32| Observation {
            chain_tx_key: key.into(),
            txid: key.into(),
            vout: Some(0),
            amount: Decimal::ONE,
            confirmations: c,
            block_height: None,
        };
        assert!(t.missing("a", &[]).is_empty());
        t.remember("a", &[o("t1", 0), o("t2", 3)]);
        assert!(t.missing("a", &[o("t1", 0)]).is_empty());
        let gone = t.missing("a", &[o("t2", 4)]);
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].chain_tx_key, "t1");
        t.remember("a", &[o("t1", 1)]);
        assert!(t.missing("a", &[]).is_empty(), "confirmed deposits are not tracked");
    }
}
