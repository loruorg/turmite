//! Monero driver over monero-wallet-rpc. View-only: the server holds no spend key.
//!
//! Each project has a view-only wallet file made from its primary address and private view
//! key (`generate_from_keys` without a spend key). A payment's deposit address is a new
//! subaddress of account 0, so the coins land in the merchant's own wallet and nothing is
//! swept. Detection is `get_transfers` (pool and chain) for that subaddress. Monero locks
//! received outputs for 10 blocks, so the network requires 10 confirmations.
//!
//! wallet-rpc holds one open wallet at a time. The driver opens the project's file before
//! each call and checks the open wallet's address, because another process (the API and the
//! worker run apart) can switch it. Its RPC must not use `--rpc-login` (digest auth is not
//! supported); keep it on a private interface.

use std::time::Duration;

use async_trait::async_trait;
use curve25519_dalek::{EdwardsPoint, Scalar, edwards::CompressedEdwardsY};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::{
    chains::{
        btc::{DROPPED, Unconfirmed},
        driver::{AssetContext, DerivedAddress, IndexScope, Observation, PaymentDriver, SweepRequest, SweepResult, WalletContext},
    },
    error::{Error, Result},
};

const PICONERO_PER_XMR: u64 = 1_000_000_000_000;

pub struct XmrDriver {
    client: reqwest::Client,
    url: String,
    /// The wallet file this process last opened in the wallet-rpc. The lock also keeps a
    /// switch from happening in the middle of a call sequence.
    open: tokio::sync::Mutex<Option<String>>,
    unconfirmed: Unconfirmed,
}

struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn into_error(self, method: &str) -> Error {
        Error::Driver(format!("wallet-rpc {method}: {} (code {})", self.message, self.code))
    }
}

/// The view-only wallet of a project, from its saved settings.
struct Keys<'a> {
    address: &'a str,
    view_key: &'a str,
    restore_height: u64,
}

fn keys(w: &WalletContext) -> Result<Keys<'_>> {
    let missing = || Error::BadRequest("This project has no Monero view-only wallet. Add it on the Wallets page.".into());
    Ok(Keys {
        address: w.address.as_deref().ok_or_else(missing)?,
        view_key: w.view_key.as_deref().map(String::as_str).ok_or_else(missing)?,
        restore_height: w.restore_height.ok_or_else(missing)?.max(0) as u64,
    })
}

/// File name of a project's view-only wallet. wallet-rpc cannot change the keys or the
/// restore height of a file, so a new address or height gets a new file.
pub fn wallet_file(project_id: i64, address: &str, restore_height: u64) -> String {
    let h = crate::crypto::sha256_hex(format!("{address}:{restore_height}").as_bytes());
    format!("turmite-p{project_id}-{}", &h[..12])
}

impl XmrDriver {
    pub async fn connect(wallet_rpc_url: &str) -> Result<Self> {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(600)).build().map_err(|e| Error::Internal(e.to_string()))?;
        let driver = Self {
            client,
            url: format!("{}/json_rpc", wallet_rpc_url.trim_end_matches('/')),
            open: Default::default(),
            unconfirmed: Default::default(),
        };
        driver.rpc("get_version", json!({})).await?;
        Ok(driver)
    }

    async fn call(&self, method: &str, params: Value) -> Result<std::result::Result<Value, RpcError>> {
        let body = json!({ "jsonrpc": "2.0", "id": "turmite", "method": method, "params": params });
        let resp = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Driver(format!("wallet-rpc {method}: {}", e.without_url())))?;
        let v: Value = resp.json().await.map_err(|e| Error::Driver(format!("wallet-rpc {method}: {}", e.without_url())))?;
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            return Ok(Err(RpcError {
                code: err["code"].as_i64().unwrap_or(0),
                message: err["message"].as_str().unwrap_or_default().to_string(),
            }));
        }
        Ok(Ok(v.get("result").cloned().unwrap_or(Value::Null)))
    }

    async fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        self.call(method, params).await?.map_err(|e| e.into_error(method))
    }

    /// Primary address of the open wallet.
    async fn primary(&self) -> Result<String> {
        let res = self.rpc("get_address", json!({ "account_index": 0, "address_index": [0] })).await?;
        res["address"].as_str().map(str::to_string).ok_or_else(|| Error::Driver("get_address: no address".into()))
    }

    /// Open the project's view-only wallet, making it on first use. The caller holds `open`.
    async fn open_wallet<'a>(&self, open: &mut Option<String>, w: &'a WalletContext) -> Result<Keys<'a>> {
        let k = keys(w)?;
        let file = wallet_file(w.project_id, k.address, k.restore_height);
        if open.as_deref() == Some(file.as_str()) && self.primary().await.ok().as_deref() == Some(k.address) {
            return Ok(k);
        }
        *open = None;
        match self.call("open_wallet", json!({ "filename": file, "password": "" })).await? {
            Ok(_) => {}
            Err(e) if e.message.contains("not found") => {
                // wallet-rpc does not check that the view key matches the address.
                check_view_key(k.address, k.view_key).map_err(Error::BadRequest)?;
                let made = self
                    .call(
                        "generate_from_keys",
                        json!({
                            "filename": file,
                            "address": k.address,
                            "viewkey": k.view_key,
                            "password": "",
                            "restore_height": k.restore_height,
                            "autosave_current": true,
                        }),
                    )
                    .await?;
                if let Err(e) = made {
                    if e.message.contains("parse") {
                        return Err(Error::BadRequest(
                            "The Monero wallet RPC does not accept this address. Check it and the network.".into(),
                        ));
                    }
                    return Err(e.into_error("generate_from_keys"));
                }
            }
            Err(e) => return Err(e.into_error("open_wallet")),
        }
        if self.primary().await? != k.address {
            return Err(Error::Driver("wallet-rpc: the open wallet has another address; trying again later".into()));
        }
        *open = Some(file);
        Ok(k)
    }

    /// Confirmations of a pool or chain transfer to `address`, or `DROPPED`.
    fn transfer_confirmations(t: &Value) -> i32 {
        if t["type"].as_str() == Some("pool") {
            return if t["double_spend_seen"].as_bool().unwrap_or(false) { DROPPED } else { 0 };
        }
        t["confirmations"].as_i64().unwrap_or(0).clamp(0, i32::MAX as i64) as i32
    }
}

fn piconero_to_xmr(p: u64) -> Decimal {
    (Decimal::from(p) / Decimal::from(PICONERO_PER_XMR)).round_dp(12)
}

#[async_trait]
impl PaymentDriver for XmrDriver {
    fn chain(&self) -> &str {
        "monero"
    }

    fn index_scope(&self) -> IndexScope {
        IndexScope::Wallet
    }

    fn needs_sweep(&self) -> bool {
        false
    }

    /// Valid for the wallet-rpc's network when a wallet is open, else for any network.
    async fn validate_address(&self, address: &str) -> Result<bool> {
        let res = match self.call("validate_address", json!({ "address": address, "any_net_type": false })).await? {
            Ok(v) => v,
            Err(_) => self.rpc("validate_address", json!({ "address": address, "any_net_type": true })).await?,
        };
        Ok(res["valid"].as_bool().unwrap_or(false))
    }

    /// Make the view-only wallet. Returns the primary address the wallet reports (equal to
    /// the saved one) and the restore height.
    async fn prepare_wallet(&self, w: &WalletContext) -> Result<Vec<String>> {
        let k = keys(w)?;
        check_view_key(k.address, k.view_key).map_err(Error::BadRequest)?;
        let mut open = self.open.lock().await;
        self.open_wallet(&mut open, w).await?;
        let primary = self.primary().await?;
        if primary != k.address {
            return Err(Error::BadRequest("The view-only wallet reports another primary address.".into()));
        }
        Ok(vec![primary, k.restore_height.to_string()])
    }

    /// A new subaddress of account 0. Its index comes from the wallet.
    async fn derive_deposit(&self, w: &WalletContext, _derivation_index: u64) -> Result<DerivedAddress> {
        let mut open = self.open.lock().await;
        let k = self.open_wallet(&mut open, w).await?;
        let res = self.rpc("create_address", json!({ "account_index": 0, "label": "turmite" })).await?;
        let address = res["address"].as_str().ok_or_else(|| Error::Driver("create_address: no address".into()))?.to_string();
        let index = res["address_index"].as_u64().ok_or_else(|| Error::Driver("create_address: no index".into()))?;
        // Another process may have switched the open wallet between the calls.
        if !is_subaddress_of(k.address, k.view_key, &address) {
            *open = None;
            return Err(Error::Driver("wallet-rpc made a subaddress of another wallet; try again".into()));
        }
        Ok(DerivedAddress { address, payin_extra_id: None, derivation_index: index })
    }

    async fn observe(
        &self,
        w: &WalletContext,
        address: &str,
        _payin_extra_id: Option<&str>,
        asset: &AssetContext,
    ) -> Result<Vec<Observation>> {
        if !asset.is_native() {
            return Err(Error::Driver("monero: token assets are not supported".into()));
        }
        let mut open = self.open.lock().await;
        let k = self.open_wallet(&mut open, w).await?;
        // Scan new blocks and the pool now instead of waiting for the auto-refresh.
        self.rpc("refresh", json!({})).await?;
        let index = match self.call("get_address_index", json!({ "address": address })).await? {
            Ok(v) => v["index"].clone(),
            Err(e) => {
                *open = None;
                return Err(e.into_error("get_address_index"));
            }
        };
        let (major, minor) = (index["major"].as_u64().unwrap_or(0), index["minor"].as_u64().unwrap_or(0));
        let res =
            self.rpc("get_transfers", json!({ "in": true, "pool": true, "account_index": major, "subaddr_indices": [minor] })).await?;
        if self.primary().await? != k.address {
            *open = None;
            return Err(Error::Driver("wallet-rpc switched wallets during the scan; trying again later".into()));
        }
        let mut out = Vec::new();
        for t in ["in", "pool"].iter().flat_map(|kind| res[*kind].as_array().cloned().unwrap_or_default()) {
            let txid = t["txid"].as_str().unwrap_or_default();
            let amount = piconero_to_xmr(t["amount"].as_u64().unwrap_or(0));
            if t["address"].as_str() != Some(address) || txid.is_empty() || amount.is_zero() {
                continue;
            }
            out.push(Observation {
                chain_tx_key: format!("{txid}:{minor}"),
                txid: txid.to_string(),
                vout: None,
                amount,
                confirmations: Self::transfer_confirmations(&t),
                block_height: t["height"].as_i64().filter(|h| *h > 0),
            });
        }
        // A pool transaction the daemon dropped disappears from the list.
        for mut o in self.unconfirmed.missing(address, &out) {
            let found = self.call("get_transfer_by_txid", json!({ "txid": o.txid, "account_index": major })).await?;
            o.confirmations = found
                .ok()
                .and_then(|v| v["transfers"].as_array().into_iter().flatten().find(|t| t["address"].as_str() == Some(address)).cloned())
                .map(|t| Self::transfer_confirmations(&t))
                .unwrap_or(DROPPED);
            out.push(o);
        }
        self.unconfirmed.remember(address, &out);
        Ok(out)
    }

    async fn sweep(&self, _w: &WalletContext, _req: &SweepRequest, _asset: &AssetContext) -> Result<SweepResult> {
        Err(Error::Driver("monero deposits land in the merchant's wallet; there is nothing to sweep".into()))
    }

    /// Height of the newest block, from the open wallet (any project's). Without an open
    /// wallet, or while a long scan holds it, the RPC is up but has no height to give.
    async fn tip_height(&self) -> Result<i64> {
        let busy = || Error::Driver("tip height is not available while the wallet scans".into());
        let _open = self.open.try_lock().map_err(|_| busy())?;
        match self.call("get_height", json!({})).await? {
            Ok(v) => Ok(v["height"].as_i64().unwrap_or(0) - 1),
            Err(_) => {
                self.rpc("get_version", json!({})).await?;
                Err(Error::Driver("tip height is not available until a project has a Monero wallet".into()))
            }
        }
    }
}

// ---- addresses and keys ------------------------------------------------------

/// A Monero network, from the address prefix. Regtest uses the mainnet prefixes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Net {
    Mainnet,
    Stagenet,
    Testnet,
}

/// (network, primary address prefix, subaddress prefix)
const PREFIXES: [(Net, u8, u8); 3] = [(Net::Mainnet, 18, 42), (Net::Stagenet, 24, 36), (Net::Testnet, 53, 63)];

const B58: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
/// Characters per block of n bytes (Monero base58 works on 8-byte blocks).
const ENCODED_SIZES: [usize; 9] = [0, 2, 3, 5, 6, 7, 9, 10, 11];

fn decode_block(chars: &[u8], len: usize) -> Option<Vec<u8>> {
    let mut num: u128 = 0;
    for c in chars {
        num = num * 58 + B58.iter().position(|b| b == c)? as u128;
    }
    if num >> (8 * len) != 0 {
        return None;
    }
    Some(num.to_be_bytes()[16 - len..].to_vec())
}

fn base58_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let rest = ENCODED_SIZES.iter().position(|&n| n == b.len() % 11)?;
    let mut out = Vec::with_capacity(b.len() * 8 / 11 + 8);
    for chunk in b.chunks(11) {
        let len = if chunk.len() == 11 { 8 } else { rest };
        out.extend(decode_block(chunk, len)?);
    }
    Some(out)
}

/// (prefix, public spend key, public view key) of a 95-character address. The checksum is
/// not checked here; the wallet-rpc checks it when it reads the address.
fn decode_address(address: &str) -> Option<(u8, [u8; 32], [u8; 32])> {
    if address.len() != 95 {
        return None;
    }
    let raw = base58_decode(address)?;
    if raw.len() != 69 || raw[0] >= 0x80 {
        return None;
    }
    Some((raw[0], raw[1..33].try_into().ok()?, raw[33..65].try_into().ok()?))
}

/// Check a primary address. On mainnet only mainnet addresses pass; on a test install
/// stagenet, testnet and regtest (mainnet prefix) pass. The error is a message for the operator.
pub fn check_primary_address(address: &str, mainnet: bool) -> std::result::Result<Net, String> {
    if address.len() == 106 {
        return Err("This is an integrated address. Enter the primary address of the wallet.".into());
    }
    if address.len() != 95 {
        return Err("A Monero primary address has 95 characters.".into());
    }
    let (prefix, _, _) = decode_address(address).ok_or("This is not a valid Monero address.")?;
    if PREFIXES.iter().any(|p| p.2 == prefix) {
        return Err("This is a subaddress. Enter the primary address of the wallet.".into());
    }
    let net = PREFIXES.iter().find(|p| p.1 == prefix).map(|p| p.0).ok_or("This is not a Monero address.")?;
    if mainnet && net != Net::Mainnet {
        return Err("This address is for a test network. This server runs on mainnet.".into());
    }
    Ok(net)
}

/// Offline check of any Monero address (primary, subaddress or integrated): base58 layout and a
/// known prefix. On mainnet only mainnet prefixes pass. The wallet-rpc checks the checksum.
pub fn address_ok(address: &str, mainnet: bool) -> bool {
    /// (network, integrated address prefix)
    const INTEGRATED: [(Net, u8); 3] = [(Net::Mainnet, 19), (Net::Stagenet, 25), (Net::Testnet, 54)];
    let (len, integrated) = match address.len() {
        95 => (69, false),
        106 => (77, true),
        _ => return false,
    };
    let Some(raw) = base58_decode(address).filter(|r| r.len() == len) else { return false };
    let net = if integrated {
        INTEGRATED.iter().find(|p| p.1 == raw[0]).map(|p| p.0)
    } else {
        PREFIXES.iter().find(|p| p.1 == raw[0] || p.2 == raw[0]).map(|p| p.0)
    };
    net.is_some_and(|n| !mainnet || n == Net::Mainnet)
}

fn scalar(view_key: &str) -> Option<Scalar> {
    let bytes: [u8; 32] = hex::decode(view_key.trim()).ok()?.try_into().ok()?;
    Option::from(Scalar::from_canonical_bytes(bytes))
}

/// Check that a private view key (64 hex characters) belongs to the address.
pub fn check_view_key(address: &str, view_key: &str) -> std::result::Result<(), String> {
    let view_key = view_key.trim();
    if view_key.len() != 64 || !view_key.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("A private view key has 64 hexadecimal characters.".into());
    }
    let a = scalar(view_key).ok_or("This is not a valid private view key.")?;
    let (_, _, view) = decode_address(address).ok_or("This is not a valid Monero address.")?;
    if EdwardsPoint::mul_base(&a).compress().to_bytes() != view {
        return Err("This view key does not belong to this address.".into());
    }
    Ok(())
}

/// True when `sub` is a subaddress made with this view key on the primary address's network:
/// its view part is the view key times its spend part. The caller has checked that the view
/// key belongs to the primary address.
pub fn is_subaddress_of(primary: &str, view_key: &str, sub: &str) -> bool {
    let (Some((p, _, _)), Some((s, d, c)), Some(a)) = (decode_address(primary), decode_address(sub), scalar(view_key)) else {
        return false;
    };
    let same_net = PREFIXES.iter().any(|n| n.1 == p && n.2 == s);
    let Some(d) = CompressedEdwardsY(d).decompress() else { return false };
    same_net && (d * a).compress().to_bytes() == c
}

#[cfg(test)]
mod tests {
    use super::*;

    // A regtest wallet (mainnet prefixes) and two of its subaddresses, from wallet-rpc 0.18.
    const ADDRESS: &str = "48x8jjw6FYQHmGRW2mKTnR83bKy7oRez9NWwVQiqkqxf4uuu9X9kKNbY1bdHKPS5Zo4968N8Ts2fMeDSPvYvkHnM5vjiiVL";
    const VIEW_KEY: &str = "5909dde10096461d2bc6ab7167268dce513aa2bd494dd93e0f0bc85afa016e0e";
    const SUB1: &str = "86vA2kzcZ4NbpYrjxTmGTwJeLJzqWbuJxajHqjbCnpvnTovPG6F7K4sEMgqqV41mfxYJ6cWx1uvphLMJEkCP3ocn9U2uFhD";
    const SUB2: &str = "86MVvJFSoYv1ZALQeCtp7b4ybUREndXZBaxaqQ7iRipEbRsCNyDATBYbfgjJEiVdz76VT3aQGAqx5Dqru6qYHnbi9vEnDp2";
    // Another wallet.
    const OTHER: &str = "45ifodRPQT71A5QN8Sc8DAMqkEnErJK1QWLA9fGEx2NLDrheJvVfz8x6a57D1Zry93gnzAN5saWK3EhQguTcDszV46r876j";

    #[test]
    fn piconero() {
        assert_eq!(piconero_to_xmr(1_500_000_000_000), Decimal::new(15, 1));
        assert_eq!(piconero_to_xmr(1), Decimal::new(1, 12));
    }

    #[test]
    fn primary_addresses() {
        assert_eq!(check_primary_address(ADDRESS, true), Ok(Net::Mainnet));
        assert_eq!(check_primary_address(ADDRESS, false), Ok(Net::Mainnet), "regtest uses mainnet prefixes");
        assert!(check_primary_address(SUB1, false).unwrap_err().contains("subaddress"));
        assert!(check_primary_address(&ADDRESS[..94], false).unwrap_err().contains("95"));
        assert!(check_primary_address(&format!("{ADDRESS}{}", &ADDRESS[..11]), false).unwrap_err().contains("integrated"));
        assert!(check_primary_address(&ADDRESS.replace('4', "0"), false).is_err(), "0 is not base58");
        // A stagenet address: prefix 24.
        let mut raw = base58_decode(ADDRESS).unwrap();
        raw[0] = 24;
        let stagenet = encode(&raw);
        assert!(stagenet.starts_with('5'));
        assert_eq!(check_primary_address(&stagenet, false), Ok(Net::Stagenet));
        assert!(check_primary_address(&stagenet, true).unwrap_err().contains("test network"));
    }

    #[test]
    fn any_address_offline() {
        for a in [ADDRESS, SUB1, OTHER] {
            assert!(address_ok(a, true) && address_ok(a, false), "{a}");
        }
        let mut raw = base58_decode(ADDRESS).unwrap();
        raw.extend([1, 2, 3, 4, 5, 6, 7, 8]);
        raw[0] = 19;
        let integrated = encode(&raw);
        assert_eq!(integrated.len(), 106);
        assert!(address_ok(&integrated, true));
        raw[0] = 54;
        let testnet_integrated = encode(&raw);
        assert!(address_ok(&testnet_integrated, false) && !address_ok(&testnet_integrated, true));
        assert!(!address_ok(&ADDRESS[..94], false));
        assert!(!address_ok(&ADDRESS.replace('4', "0"), false));
        let mut raw = base58_decode(ADDRESS).unwrap();
        raw[0] = 77;
        assert!(!address_ok(&encode(&raw), false), "unknown prefix");
    }

    #[test]
    fn view_keys() {
        assert_eq!(check_view_key(ADDRESS, VIEW_KEY), Ok(()));
        assert_eq!(check_view_key(ADDRESS, &format!(" {VIEW_KEY}\n")), Ok(()));
        assert!(check_view_key(OTHER, VIEW_KEY).unwrap_err().contains("does not belong"));
        assert!(check_view_key(ADDRESS, &VIEW_KEY[..63]).unwrap_err().contains("64"));
        assert!(check_view_key(ADDRESS, &"f".repeat(64)).unwrap_err().contains("not a valid"), "not a canonical scalar");
        let mut wrong = VIEW_KEY.to_string();
        wrong.replace_range(0..1, "6");
        assert!(check_view_key(ADDRESS, &wrong).is_err());
    }

    #[test]
    fn subaddresses() {
        assert!(is_subaddress_of(ADDRESS, VIEW_KEY, SUB1));
        assert!(is_subaddress_of(ADDRESS, VIEW_KEY, SUB2));
        let other_key = format!("6{}", &VIEW_KEY[1..]);
        assert!(!is_subaddress_of(ADDRESS, &other_key, SUB1), "made with another view key");
        assert!(!is_subaddress_of(ADDRESS, VIEW_KEY, OTHER), "a primary address is not a subaddress");
    }

    #[test]
    fn file_names_follow_the_keys() {
        assert_eq!(wallet_file(7, ADDRESS, 100), wallet_file(7, ADDRESS, 100));
        assert_ne!(wallet_file(7, ADDRESS, 100), wallet_file(7, ADDRESS, 101));
        assert!(wallet_file(7, ADDRESS, 100).starts_with("turmite-p7-"));
    }

    fn encode(raw: &[u8]) -> String {
        let mut out = String::new();
        for chunk in raw.chunks(8) {
            let mut num = chunk.iter().fold(0u128, |n, b| n * 256 + *b as u128);
            let size = ENCODED_SIZES[chunk.len()];
            let mut block = vec![b'1'; size];
            for i in (0..size).rev() {
                block[i] = B58[(num % 58) as usize];
                num /= 58;
            }
            out.push_str(std::str::from_utf8(&block).unwrap());
        }
        out
    }
}
