//! TON driver (native TON) over the toncenter HTTP API v2.
//!
//! Per-payment deposit address = a wallet v4r2 contract address derived from
//! the deposit seed + index (ed25519). It stays undeployed until the first
//! sweep, so it is shown non-bounceable: wallets then send to it without the
//! funds bouncing back. Detection reads the address's incoming transactions
//! (one observation per transaction). The sweep sends one signed external
//! message that deploys the wallet when needed (StateInit) and sends the whole
//! balance (mode 128) to the settlement address, with the settlement memo as a
//! text comment. The sweep message is non-bounceable: a bounce would bring the
//! funds back to the deposit, where they would count as a new payment.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ed25519_dalek::SigningKey;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::Value;
use sha2::{Digest, Sha512};
use tonlib_core::TonAddress;
use tonlib_core::cell::{ArcCell, BagOfCells, CellBuilder, TonCellError};
use tonlib_core::tlb_types::tlb::TLB;
use tonlib_core::wallet::mnemonic::KeyPair;
use tonlib_core::wallet::ton_wallet::TonWallet;
use tonlib_core::wallet::versioned::DEFAULT_WALLET_ID;
use tonlib_core::wallet::versioned::v4::WalletExtMsgBodyV4;
use tonlib_core::wallet::wallet_version::WalletVersion;
use zeroize::Zeroizing;

use crate::{
    chains::driver::{
        AssetContext, DerivedAddress, Observation, PaymentDriver, SweepCheck, SweepRequest, SweepResult, SweepState, WalletContext,
    },
    error::{Error, Result},
};

const NANOTON_PER_TON: u64 = 1_000_000_000;
/// Do not sweep balances that would mostly go to fees (0.01 TON).
const MIN_SWEEP_NANOTON: u64 = 10_000_000;
/// Send all remaining balance (fees come out of the value).
const MODE_CARRY_BALANCE: u8 = 128;
/// The sender pays the forward fee on top of the value.
const MODE_PAY_FEES_SEPARATELY: u8 = 1;
/// Root hashes of the masterchain zerostates of the public networks.
const MAINNET_ZEROSTATE: &str = "F6OpKZKqvqeFp6CQmFomXNMfMj2EnaUSOXN+Mh+wVWk=";
const TESTNET_ZEROSTATE: &str = "gj+B8wb/AmlPk1z1AhVI484rhrUpgSr2oSFIh56VoSg=";

pub struct TonDriver {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    master_seed: Option<Zeroizing<String>>,
    testnet: bool,
    confirmations_required: i32,
}

impl TonDriver {
    /// `url` is the API v2 root, e.g. `https://testnet.toncenter.com/api/v2`. An
    /// `api_key` query parameter is sent as the `X-API-Key` header. Without
    /// `master_seed` (the API process), the driver watches and validates but cannot
    /// derive or sweep. Refuses a node on another network than the mode's (see
    /// `check_zerostate`).
    pub async fn connect(url: &str, master_seed: Option<&str>, testnet: bool, confirmations_required: i32) -> Result<Self> {
        let (base_url, api_key) = crate::chains::registry::split_api_key(url);
        let driver = Self {
            client: reqwest::Client::builder().timeout(Duration::from_secs(20)).build().map_err(|e| Error::Internal(e.to_string()))?,
            base_url,
            api_key,
            master_seed: master_seed.map(|s| Zeroizing::new(s.to_string())),
            testnet,
            confirmations_required,
        };
        let info = driver.get("getMasterchainInfo", &[]).await?;
        check_zerostate(info.pointer("/init/root_hash").and_then(Value::as_str).unwrap_or_default(), testnet)?;
        Ok(driver)
    }

    async fn get(&self, method: &str, query: &[(&str, &str)]) -> Result<Value> {
        self.call(method, || self.client.get(format!("{}/{method}", self.base_url)).query(query)).await
    }

    async fn post(&self, method: &str, body: Value) -> Result<Value> {
        self.call(method, || self.client.post(format!("{}/{method}", self.base_url)).json(&body)).await
    }

    /// Send with up to 4 tries: toncenter answers 429 above its rate limit
    /// (1 request/s without a key), and connections can drop.
    async fn call(&self, method: &str, request: impl Fn() -> reqwest::RequestBuilder) -> Result<Value> {
        let mut last = String::new();
        for attempt in 0..4u64 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(1100 * attempt)).await;
            }
            let mut req = request();
            if let Some(key) = &self.api_key {
                req = req.header("X-API-Key", key);
            }
            let resp = match req.send().await {
                Ok(r) if r.status().as_u16() == 429 || r.status().is_server_error() => {
                    last = format!("HTTP {}", r.status());
                    continue;
                }
                Ok(r) => r,
                Err(e) => {
                    last = e.to_string();
                    continue;
                }
            };
            let v: Value = resp.json().await.map_err(|e| Error::Driver(format!("ton {method} decode: {e}")))?;
            if v.get("ok").and_then(Value::as_bool) != Some(true) {
                return Err(Error::Driver(format!("ton {method}: {v}")));
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(Error::Driver(format!("ton {method}: {last}")))
    }

    fn key_pair(&self, index: u64) -> Result<KeyPair> {
        let master =
            self.master_seed.as_deref().ok_or_else(|| Error::Driver("ton: the deposit seed is only available in the worker".into()))?;
        let mut h = Sha512::new();
        h.update(master.as_bytes());
        h.update(b"/ton/");
        h.update(index.to_le_bytes());
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&h.finalize()[..32]);
        Ok(key_pair_from_seed(&seed))
    }

    fn wallet(&self, index: u64) -> Result<TonWallet> {
        TonWallet::new_with_params(WalletVersion::V4R2, self.key_pair(index)?, 0, DEFAULT_WALLET_ID)
            .map_err(|e| Error::Driver(format!("ton wallet: {e}")))
    }

    fn friendly(&self, address: &TonAddress) -> String {
        address.to_base64_url_flags(true, self.testnet)
    }

    /// (balance in nanoton, seqno, deployed) of a wallet address.
    async fn wallet_state(&self, address: &str) -> Result<(u64, u32, bool)> {
        let info = self.get("getWalletInformation", &[("address", address)]).await?;
        let balance = parse_u64(info.get("balance"));
        let deployed = info.get("account_state").and_then(Value::as_str) == Some("active");
        let seqno = info.get("seqno").and_then(Value::as_u64).unwrap_or(0) as u32;
        Ok((balance, seqno, deployed))
    }

    async fn transactions(&self, address: &str, limit: u32) -> Result<Vec<Value>> {
        let limit = limit.to_string();
        // Recent transactions only: deposits are new, and public APIs often
        // have no archival workers.
        let res = self.get("getTransactions", &[("address", address), ("limit", &limit)]).await?;
        Ok(res.as_array().cloned().unwrap_or_default())
    }

    /// Sign `msgs` (with their send modes) from `wallet` and broadcast them.
    /// Returns the hex hash of the external message.
    async fn send(&self, wallet: &TonWallet, seqno: u32, deploy: bool, msgs: Vec<ArcCell>, modes: Vec<u8>) -> Result<String> {
        let valid_until = (chrono::Utc::now().timestamp() + 120) as u32;
        let body = match wallet.version {
            WalletVersion::V4R2 => {
                WalletExtMsgBodyV4 { subwallet_id: wallet.wallet_id, valid_until, msg_seqno: seqno, opcode: 0, msgs_modes: modes, msgs }
                    .to_cell()
            }
            // Other versions (test funders): the library default mode 3.
            _ => wallet.create_external_body(valid_until, seqno, msgs),
        }
        .map_err(|e| Error::Driver(format!("ton body: {e}")))?;
        let signed = wallet.sign_external_body(&body).map_err(|e| Error::Driver(format!("ton sign: {e}")))?;
        let external = wallet.wrap_signed_body(signed, deploy).map_err(|e| Error::Driver(format!("ton message: {e}")))?;
        let boc = BagOfCells::from_root(external.clone()).serialize(true).map_err(|e| Error::Driver(format!("ton boc: {e}")))?;
        use base64::Engine;
        let boc = base64::engine::general_purpose::STANDARD.encode(boc);
        self.post("sendBoc", serde_json::json!({ "boc": boc })).await?;
        Ok(external.cell_hash().to_hex())
    }

    /// Address of a wallet given by its ed25519 secret (hex). For funding and tests.
    pub fn address_from_secret(&self, secret_hex: &str, version: WalletVersion, wallet_id: i32, workchain: i32) -> Result<String> {
        Ok(self.friendly(&secret_wallet(secret_hex, version, wallet_id, workchain)?.address))
    }

    /// Send `nanoton` from a wallet given by its ed25519 secret (hex). For
    /// funding and tests; `version`/`wallet_id`/`workchain` describe the wallet.
    pub async fn send_from_secret(
        &self,
        secret_hex: &str,
        version: WalletVersion,
        wallet_id: i32,
        workchain: i32,
        to: &str,
        nanoton: u64,
    ) -> Result<String> {
        let wallet = secret_wallet(secret_hex, version, wallet_id, workchain)?;
        let (_, seqno, deployed) = self.wallet_state(&wallet.address.to_hex()).await?;
        let to = parse_address(to)?;
        let msg = internal_message(&to, nanoton, None)?;
        self.send(&wallet, seqno, !deployed, vec![msg], vec![MODE_PAY_FEES_SEPARATELY]).await
    }

    /// Balance of an address in nanoton.
    pub async fn balance_nanoton(&self, address: &str) -> Result<u64> {
        Ok(parse_u64(Some(&self.get("getAddressBalance", &[("address", address)]).await?)))
    }
}

/// A wallet from its ed25519 secret (hex), for funding and tests.
fn secret_wallet(secret_hex: &str, version: WalletVersion, wallet_id: i32, workchain: i32) -> Result<TonWallet> {
    let bytes = hex::decode(secret_hex.trim_start_matches("0x")).map_err(|e| Error::Driver(e.to_string()))?;
    let seed: [u8; 32] =
        bytes.get(..32).and_then(|b| b.try_into().ok()).ok_or_else(|| Error::Driver("ton secret must be 32 bytes".into()))?;
    TonWallet::new_with_params(version, key_pair_from_seed(&seed), workchain, wallet_id)
        .map_err(|e| Error::Driver(format!("ton wallet: {e}")))
}

/// Mainnet mode accepts only a mainnet node. Testnet mode accepts the public testnet
/// and an unknown chain (a local network has its own zerostate), but not mainnet.
fn check_zerostate(root_hash: &str, testnet: bool) -> Result<()> {
    let network = match root_hash {
        MAINNET_ZEROSTATE => "mainnet",
        TESTNET_ZEROSTATE => "testnet",
        _ => "an unknown network",
    };
    match (testnet, root_hash == MAINNET_ZEROSTATE) {
        (false, true) | (true, false) => Ok(()),
        (false, false) => Err(Error::Driver(format!("ton: this node is on {network}, not on mainnet (zerostate {root_hash})"))),
        (true, true) => Err(Error::Driver("ton: this node is on mainnet, but this install is in testnet mode".into())),
    }
}

fn key_pair_from_seed(seed: &[u8; 32]) -> KeyPair {
    let public = SigningKey::from_bytes(seed).verifying_key().to_bytes();
    // NaCl secret-key layout, as tonlib-core signs with: seed || public key.
    let mut secret = seed.to_vec();
    secret.extend_from_slice(&public);
    KeyPair { public_key: public.to_vec(), secret_key: secret }
}

fn parse_address(address: &str) -> Result<TonAddress> {
    TonAddress::from_str(address.trim()).map_err(|e| Error::Driver(format!("bad ton address {address}: {e}")))
}

fn parse_u64(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

/// `Coins` (VarUInteger 16): 4-bit byte count, then the value.
fn store_coins(b: &mut CellBuilder, nanoton: u64) -> std::result::Result<(), TonCellError> {
    let bytes = (64 - nanoton.leading_zeros() as usize).div_ceil(8);
    b.store_u8(4, bytes as u8)?;
    if bytes > 0 {
        b.store_u64(bytes * 8, nanoton)?;
    }
    Ok(())
}

/// Text comment body: op 0, then UTF-8 text, continued in child cells (snake format).
fn comment_body(text: &str) -> std::result::Result<ArcCell, TonCellError> {
    let bytes = text.as_bytes();
    // 127 bytes fit in a cell; the first cell also holds the 4-byte op.
    let first = bytes.len().min(123);
    let mut chunks: Vec<&[u8]> = vec![&bytes[..first]];
    chunks.extend(bytes[first..].chunks(127));
    let mut child: Option<ArcCell> = None;
    for (i, chunk) in chunks.iter().enumerate().rev() {
        let mut b = CellBuilder::new();
        if i == 0 {
            b.store_u32(32, 0)?;
        }
        b.store_slice(chunk)?;
        if let Some(c) = &child {
            b.store_reference(c)?;
        }
        child = Some(Arc::new(b.build()?));
    }
    Ok(child.expect("at least one cell"))
}

/// A non-bounceable value transfer (int_msg_info), with an optional text comment.
fn internal_message(to: &TonAddress, nanoton: u64, comment: Option<&str>) -> Result<ArcCell> {
    let build = || -> std::result::Result<ArcCell, TonCellError> {
        let mut b = CellBuilder::new();
        b.store_bit(false)?; // internal message
        b.store_bit(true)?; // ihr_disabled
        b.store_bit(false)?; // bounce: the target may be an undeployed wallet
        b.store_bit(false)?; // bounced
        b.store_address(&TonAddress::NULL)?; // src: set by the sender's wallet
        b.store_address(to)?;
        store_coins(&mut b, nanoton)?;
        b.store_bit(false)?; // no extra currencies
        store_coins(&mut b, 0)?; // ihr_fee
        store_coins(&mut b, 0)?; // fwd_fee
        b.store_u64(64, 0)?; // created_lt
        b.store_u64(32, 0)?; // created_at
        b.store_bit(false)?; // no state_init
        match comment.filter(|c| !c.is_empty()) {
            Some(text) => {
                b.store_bit(true)?; // body in a reference
                b.store_reference(&comment_body(text)?)?;
            }
            None => {
                b.store_bit(false)?; // empty body, inline
            }
        }
        Ok(Arc::new(b.build()?))
    };
    build().map_err(|e| Error::Driver(format!("ton message: {e}")))
}

fn nanoton_to_ton(n: u64) -> Decimal {
    (Decimal::from(n) / Decimal::from(NANOTON_PER_TON)).round_dp(9)
}

#[allow(dead_code)]
fn ton_to_nanoton(d: Decimal) -> u64 {
    (d * Decimal::from(NANOTON_PER_TON)).floor().to_u64().unwrap_or(0)
}

#[async_trait]
impl PaymentDriver for TonDriver {
    fn chain(&self) -> &str {
        "ton"
    }

    fn can_derive(&self) -> bool {
        self.master_seed.is_some()
    }

    async fn validate_address(&self, address: &str) -> Result<bool> {
        Ok(parse_address(address).is_ok())
    }

    async fn derive_deposit(&self, _w: &WalletContext, derivation_index: u64) -> Result<DerivedAddress> {
        let wallet = self.wallet(derivation_index)?;
        Ok(DerivedAddress { address: self.friendly(&wallet.address), payin_extra_id: None, derivation_index })
    }

    async fn observe(
        &self,
        _w: &WalletContext,
        address: &str,
        _payin_extra_id: Option<&str>,
        asset: &AssetContext,
    ) -> Result<Vec<Observation>> {
        if !asset.is_native() {
            return Err(Error::Driver("ton: jettons are not supported".into()));
        }
        let mut out = Vec::new();
        for tx in self.transactions(address, 50).await? {
            let Some(msg) = tx.get("in_msg") else { continue };
            // External messages (our own sweeps) have no source.
            if msg.get("source").and_then(Value::as_str).unwrap_or_default().is_empty() {
                continue;
            }
            let value = parse_u64(msg.get("value"));
            let hash = tx.pointer("/transaction_id/hash").and_then(Value::as_str).unwrap_or_default();
            if value == 0 || hash.is_empty() {
                continue;
            }
            out.push(Observation {
                chain_tx_key: hash.to_string(),
                txid: hash.to_string(),
                vout: None,
                amount: nanoton_to_ton(value),
                // Shard blocks are final once the masterchain references them.
                confirmations: self.confirmations_required.max(1),
                block_height: None,
            });
        }
        Ok(out)
    }

    async fn sweep(&self, w: &WalletContext, req: &SweepRequest, _asset: &AssetContext) -> Result<SweepResult> {
        let wallet = self.wallet(req.derivation_index)?;
        if parse_address(&req.from_address).ok().as_ref() != Some(&wallet.address) {
            return Err(Error::Driver(format!("ton: index {} does not derive {}", req.derivation_index, req.from_address)));
        }
        let from = wallet.address.to_hex();
        let (balance, seqno, deployed) = self.wallet_state(&from).await?;
        if balance < MIN_SWEEP_NANOTON {
            return Err(Error::Driver(format!("ton: balance {balance} nanoton is too small to sweep")));
        }
        let last_lt = self
            .transactions(&from, 1)
            .await?
            .first()
            .and_then(|t| t.pointer("/transaction_id/lt"))
            .map(|v| parse_u64(Some(v)))
            .unwrap_or(0);

        let mut msgs = Vec::new();
        let mut modes = Vec::new();
        let memo = req.to_extra_id.as_deref().or(w.address_extra_id.as_deref());
        msgs.push(internal_message(&parse_address(&req.to_address)?, 0, memo)?);
        modes.push(MODE_CARRY_BALANCE);
        let msg_hash = self.send(&wallet, seqno, !deployed, msgs, modes).await?;

        // Wait for the wallet transaction, to report its hash and real cost.
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_secs(2)).await;
            // The message is out: from here on, errors must not fail the sweep.
            let Ok(txs) = self.transactions(&from, 5).await else { continue };
            // Our transaction: newer than the last one before the send, and
            // started by an external message (incoming payments have a source).
            let Some(tx) = txs.into_iter().find(|t| {
                t.pointer("/transaction_id/lt").map(|v| parse_u64(Some(v))).unwrap_or(0) > last_lt
                    && t.pointer("/in_msg/source").and_then(Value::as_str).unwrap_or_default().is_empty()
            }) else {
                continue;
            };
            let sent: u64 = tx.get("out_msgs").and_then(Value::as_array).into_iter().flatten().map(|m| parse_u64(m.get("value"))).sum();
            if sent == 0 {
                return Err(Error::Driver(format!("ton sweep {msg_hash}: the wallet sent nothing")));
            }
            let txid = tx.pointer("/transaction_id/hash").and_then(Value::as_str).unwrap_or(&msg_hash).to_string();
            let network_fee = balance.saturating_sub(sent);
            return Ok(SweepResult { txid, network_fee: nanoton_to_ton(network_fee) });
        }
        // Broadcast but not yet seen: report the message hash. A retry cannot
        // double-spend, because the seqno in this message is now used.
        tracing::warn!(%msg_hash, "ton sweep not confirmed within 60s");
        Ok(SweepResult { txid: msg_hash, network_fee: Decimal::ZERO })
    }

    /// A sweep recorded with its wallet transaction hash (base64) was on chain with funds sent:
    /// final. One recorded with the hex hash of its external message is looked up by that hash.
    async fn sweep_state(&self, sweep: &SweepCheck, _asset: &AssetContext) -> Result<SweepState> {
        let Ok(msg_hash) = hex::decode(&sweep.txid) else { return Ok(SweepState::Confirmed) };
        let from = parse_address(&sweep.from_address)?.to_hex();
        for tx in self.transactions(&from, 20).await? {
            use base64::Engine;
            let hash =
                tx.pointer("/in_msg/hash").and_then(Value::as_str).and_then(|h| base64::engine::general_purpose::STANDARD.decode(h).ok());
            if hash.as_deref() == Some(msg_hash.as_slice()) {
                let sent: u64 = tx.get("out_msgs").and_then(Value::as_array).into_iter().flatten().map(|m| parse_u64(m.get("value"))).sum();
                return Ok(if sent > 0 { SweepState::Confirmed } else { SweepState::Failed("the wallet sent nothing".into()) });
            }
        }
        Ok(if sweep.expired() { SweepState::Failed("the message did not land before it expired".into()) } else { SweepState::Pending })
    }

    async fn tip_height(&self) -> Result<i64> {
        let info = self.get("getMasterchainInfo", &[]).await?;
        info.pointer("/last/seqno").and_then(Value::as_i64).ok_or_else(|| Error::Driver(format!("ton: no seqno in {info}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn driver() -> TonDriver {
        TonDriver {
            client: reqwest::Client::new(),
            base_url: String::new(),
            api_key: None,
            master_seed: Some(Zeroizing::new("unit-test-seed".into())),
            testnet: true,
            confirmations_required: 1,
        }
    }

    #[tokio::test]
    async fn derives_distinct_valid_addresses() {
        let d = driver();
        let w = WalletContext::default();
        let a0 = d.derive_deposit(&w, 0).await.unwrap().address;
        let a1 = d.derive_deposit(&w, 1).await.unwrap().address;
        assert_ne!(a0, a1);
        assert_eq!(a0.len(), 48, "user-friendly form: {a0}");
        assert!(a0.starts_with("0Q"), "testnet non-bounceable basechain address: {a0}");
        assert!(d.validate_address(&a0).await.unwrap());
        assert!(d.validate_address(&parse_address(&a0).unwrap().to_hex()).await.unwrap());
        assert!(!d.validate_address("EQnotanaddress").await.unwrap());
        assert_eq!(a0, d.derive_deposit(&w, 0).await.unwrap().address, "deterministic");
    }

    #[test]
    fn key_pair_matches_nacl_layout() {
        // A known wallet: its address must come out of our key layout.
        let seed = hex::decode("a51e8fb6f0fae3834bf430f5012589d319e7b3b3303ceb82c816b762fccf2d05").unwrap();
        let kp = key_pair_from_seed(&seed.try_into().unwrap());
        let w = TonWallet::new_with_params(WalletVersion::V3R2, kp, -1, 42).unwrap();
        assert_eq!(w.address.to_hex(), "-1:22f53b7d9aba2cef44755f7078b01614cd4dde2388a1729c2c386cf8f9898afe");
    }

    #[test]
    fn comments_are_snake_encoded() {
        let short = comment_body("123456").unwrap();
        assert_eq!(short.bit_len(), 32 + 48);
        assert_eq!(short.references().len(), 0);
        let long = "x".repeat(300);
        let cell = comment_body(&long).unwrap();
        assert_eq!(cell.bit_len(), (4 + 123) * 8);
        let second = &cell.references()[0];
        assert_eq!(second.bit_len(), 127 * 8);
        assert_eq!(second.references()[0].bit_len(), (300 - 123 - 127) * 8);
    }

    #[test]
    fn zerostate_must_match_the_mode() {
        let local = "N8Ub/6pyVBKrpQqghJ+qdyg2THrhUHNGTgO3pqTwarM=";
        assert!(check_zerostate(MAINNET_ZEROSTATE, false).is_ok());
        for other in [TESTNET_ZEROSTATE, local, ""] {
            assert!(check_zerostate(other, false).is_err(), "{other} on mainnet");
            assert!(check_zerostate(other, true).is_ok(), "{other} on testnet");
        }
        assert!(check_zerostate(MAINNET_ZEROSTATE, true).is_err());
    }

    #[test]
    fn nanoton_round_trip() {
        assert_eq!(nanoton_to_ton(1_500_000_000), Decimal::new(15, 1));
        assert_eq!(ton_to_nanoton(Decimal::new(15, 1)), 1_500_000_000);
    }
}
