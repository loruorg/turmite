//! TON payouts (native TON) from a wallet v4r2 hot wallet, over the toncenter API v2.
//!
//! One external message per payout, with the wallet's seqno. The message is valid for a few minutes. When it
//! expired and the seqno did not move, it can never run, so it is signed again. A memo goes in a text comment.

use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::SigningKey;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::PgConnection;
use tonlib_core::TonAddress;
use tonlib_core::cell::{ArcCell, BagOfCells, CellBuilder, TonCellError};
use tonlib_core::tlb_types::tlb::TLB;
use tonlib_core::wallet::mnemonic::KeyPair;
use tonlib_core::wallet::ton_wallet::TonWallet;
use tonlib_core::wallet::versioned::DEFAULT_WALLET_ID;
use tonlib_core::wallet::versioned::v4::WalletExtMsgBodyV4;
use tonlib_core::wallet::wallet_version::WalletVersion;

use crate::{
    db::{AttemptRow, NewAttempt, PgStore},
    domain::AssetNetwork,
    error::{Error, Result},
    payouts::{
        chain::{Broadcast, Landing, PayoutChain, SignCtx, SignMode, TxState},
        from_base_units,
        rpc::{Nodes, as_u64, get_json, post_json},
        to_base_units,
    },
};

const DECIMALS: u32 = 9;
const VALID_SECS: i64 = 180;
/// Send mode: the wallet pays the forward fee on top of the value.
const MODE_PAY_FEES_SEPARATELY: u8 = 1;
/// Reserve for the fees of one transfer (deploy included).
const FEE_RESERVE_NANOTON: u128 = 50_000_000;
/// Typical cost of a v4r2 transfer.
const TYPICAL_FEE_NANOTON: u128 = 6_000_000;

pub struct Ton;

fn key_pair(seed: &[u8; 32]) -> KeyPair {
    let public = SigningKey::from_bytes(seed).verifying_key().to_bytes();
    let mut secret = seed.to_vec();
    secret.extend_from_slice(&public);
    KeyPair { public_key: public.to_vec(), secret_key: secret }
}

fn wallet(seed: &[u8; 32]) -> Result<TonWallet> {
    TonWallet::new_with_params(WalletVersion::V4R2, key_pair(seed), 0, DEFAULT_WALLET_ID)
        .map_err(|e| Error::Internal(format!("ton wallet: {e}")))
}

pub fn parse_address(s: &str) -> Result<TonAddress> {
    TonAddress::from_str(s.trim()).map_err(|_| Error::BadRequest(format!("`{s}` is not a TON address.")))
}

/// Non-bounceable user-friendly address of a v4r2 wallet for an ed25519 seed: funds sent to it before it is
/// deployed stay there.
pub fn address_from_secret(secret: &[u8], testnet: bool) -> Result<String> {
    let seed: [u8; 32] = secret.try_into().map_err(|_| Error::BadRequest("A TON key has a 32-byte seed.".into()))?;
    Ok(wallet(&seed)?.address.to_base64_url_flags(true, testnet))
}

/// Seed of a 24-word TON mnemonic.
pub fn seed_from_mnemonic(words: &str) -> Result<[u8; 32]> {
    let m = tonlib_core::wallet::mnemonic::Mnemonic::from_str(words.trim(), &None)
        .map_err(|_| Error::BadRequest("Not a valid 24-word TON mnemonic.".into()))?;
    let kp = m.to_key_pair().map_err(|_| Error::BadRequest("Not a valid 24-word TON mnemonic.".into()))?;
    kp.secret_key.get(..32).and_then(|s| s.try_into().ok()).ok_or_else(|| Error::BadRequest("Not a valid TON key.".into()))
}

fn store_coins(b: &mut CellBuilder, nanoton: u128) -> std::result::Result<(), TonCellError> {
    let bytes = (128 - nanoton.leading_zeros() as usize).div_ceil(8);
    b.store_u8(4, bytes as u8)?;
    if bytes > 0 {
        let v = u64::try_from(nanoton).map_err(|_| TonCellError::InternalError("amount too large".into()))?;
        b.store_u64(bytes * 8, v)?;
    }
    Ok(())
}

/// Non-bounceable transfer with an optional text comment.
fn internal_message(to: &TonAddress, nanoton: u128, comment: Option<&str>) -> Result<ArcCell> {
    let build = || -> std::result::Result<ArcCell, TonCellError> {
        let body = match comment.filter(|c| !c.is_empty()) {
            Some(text) => {
                let mut c = CellBuilder::new();
                c.store_u32(32, 0)?;
                c.store_slice(text.as_bytes())?;
                Some(Arc::new(c.build()?))
            }
            None => None,
        };
        let mut b = CellBuilder::new();
        b.store_bit(false)?; // internal message
        b.store_bit(true)?; // ihr_disabled
        b.store_bit(false)?; // bounce off: the recipient may be an undeployed wallet
        b.store_bit(false)?; // bounced
        b.store_address(&TonAddress::NULL)?;
        b.store_address(to)?;
        store_coins(&mut b, nanoton)?;
        b.store_bit(false)?; // no extra currencies
        store_coins(&mut b, 0)?; // ihr_fee
        store_coins(&mut b, 0)?; // fwd_fee
        b.store_u64(64, 0)?; // created_lt
        b.store_u32(32, 0)?; // created_at
        b.store_bit(false)?; // no state_init
        match body {
            Some(cell) => {
                b.store_bit(true)?; // body in a reference
                b.store_reference(&cell)?;
            }
            None => {
                b.store_bit(false)?;
            }
        }
        Ok(Arc::new(b.build()?))
    };
    build().map_err(|e| Error::Internal(format!("ton message: {e}")))
}

/// A hex hash in standard base64 with padding, as toncenter writes hashes.
fn std_b64(hex_hash: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(hex::decode(hex_hash).unwrap_or_default())
}

/// The bytes of a base64 hash in either alphabet.
fn b64_bytes(s: &str) -> Option<Vec<u8>> {
    use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
    STANDARD.decode(s).ok().or_else(|| URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).ok())
}

fn same_hash(a: Option<&str>, b: &str) -> bool {
    match (a.and_then(b64_bytes), b64_bytes(b)) {
        (Some(x), Some(y)) => !x.is_empty() && x == y,
        _ => false,
    }
}

async fn wallet_state(nodes: &Nodes, address: &str) -> Result<(u128, u32, bool)> {
    let v = nodes.first_ok(|u| async move { get_json(&nodes.http, &u, "/getWalletInformation", &[("address", address)]).await }).await?;
    if v.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(Error::Driver(format!("getWalletInformation: {v}")));
    }
    let r = v.get("result").cloned().unwrap_or(Value::Null);
    let balance = as_u64(r.get("balance")).unwrap_or(0) as u128;
    let active = r.get("account_state").and_then(Value::as_str) == Some("active");
    let seqno = as_u64(r.get("seqno")).unwrap_or(0) as u32;
    Ok((balance, seqno, active))
}

async fn transactions(http: &reqwest::Client, url: &str, address: &str, limit: u32) -> Result<Vec<Value>> {
    let limit = limit.to_string();
    let v = get_json(http, url, "/getTransactions", &[("address", address), ("limit", &limit)]).await?;
    if v.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(Error::Driver(format!("getTransactions: {v}")));
    }
    Ok(v.get("result").and_then(Value::as_array).cloned().unwrap_or_default())
}

#[async_trait]
impl PayoutChain for Ton {
    async fn sign(&self, ctx: &SignCtx<'_>, conn: &mut PgConnection, _mode: SignMode<'_>) -> Result<NewAttempt> {
        let nodes = ctx.nodes;
        let seed: [u8; 32] = ctx.key.secret.as_slice().try_into().map_err(|_| Error::Internal("hot key is not 32 bytes".into()))?;
        let w = wallet(&seed)?;
        let testnet = nodes.network.is_testnet;
        if w.address.to_base64_url_flags(true, testnet) != ctx.key.address
            && w.address.to_hex() != parse_address(&ctx.key.address)?.to_hex()
        {
            return Err(Error::Internal("the hot key does not match its address".into()));
        }
        let raw_addr = w.address.to_hex();
        let to = parse_address(&ctx.payout.address)?;
        let amount = to_base_units(ctx.payout.amount, ctx.an.decimals as u32)?;
        if ctx.an.contract_address.is_some() {
            return Err(Error::BadRequest("Jetton payouts are not supported.".into()));
        }
        if ctx.payout.extra_id.as_ref().is_some_and(|m| m.len() > 120) {
            return Err(Error::BadRequest("A TON comment can have at most 120 bytes.".into()));
        }
        let (balance, chain_seqno, active) = wallet_state(nodes, &raw_addr).await?;
        // Another payout of this wallet may be signed but not yet run: use the next seqno.
        let stored = PgStore::max_open_nonce(conn, nodes.network.network_id, &ctx.key.address).await?;
        if stored.is_some_and(|s| s >= chain_seqno as i64) {
            return Err(Error::Unavailable("An earlier payout of this hot wallet is still on its way.".into()));
        }
        let need = amount + FEE_RESERVE_NANOTON;
        if balance < need {
            return Err(Error::BadRequest(format!(
                "The hot wallet holds {} TON. This payout needs about {} TON with the network fee.",
                from_base_units(balance, DECIMALS),
                from_base_units(need, DECIMALS)
            )));
        }
        let valid_until = Utc::now().timestamp() + VALID_SECS;
        let msg = internal_message(&to, amount, ctx.payout.extra_id.as_deref())?;
        let body = WalletExtMsgBodyV4 {
            subwallet_id: w.wallet_id,
            valid_until: valid_until as u32,
            msg_seqno: chain_seqno,
            opcode: 0,
            msgs_modes: vec![MODE_PAY_FEES_SEPARATELY | 2],
            msgs: vec![msg],
        }
        .to_cell()
        .map_err(|e| Error::Internal(format!("ton body: {e}")))?;
        let signed = w.sign_external_body(&body).map_err(|e| Error::Internal(format!("ton sign: {e}")))?;
        let body_hash = std_b64(&signed.cell_hash().to_hex());
        let external = w.wrap_signed_body(signed, !active).map_err(|e| Error::Internal(format!("ton message: {e}")))?;
        let boc = BagOfCells::from_root(external.clone()).serialize(true).map_err(|e| Error::Internal(format!("ton boc: {e}")))?;
        Ok(NewAttempt {
            from_address: ctx.key.address.clone(),
            txid: external.cell_hash().to_hex(),
            raw_tx: base64::engine::general_purpose::STANDARD.encode(boc),
            nonce: Some(chain_seqno as i64),
            params: json!({ "raw_address": raw_addr, "body_hash": body_hash, "msg_hash": std_b64(&external.cell_hash().to_hex()) }),
            valid_until: DateTime::<Utc>::from_timestamp(valid_until, 0),
        })
    }

    async fn broadcast(&self, nodes: &Nodes, a: &AttemptRow) -> Broadcast {
        let mut refusals = Vec::new();
        let mut unreachable = Vec::new();
        for url in &nodes.urls {
            match post_json(&nodes.http, url, "/sendBoc", &json!({ "boc": a.raw_tx })).await {
                Ok(v) if v.get("ok").and_then(Value::as_bool) == Some(true) => return Broadcast::Accepted,
                Ok(v) => {
                    let msg = v.get("error").and_then(Value::as_str).unwrap_or_default().to_string();
                    let code = as_u64(v.get("code")).unwrap_or(0);
                    if code == 429 || code >= 500 {
                        unreachable.push(format!("{code} {msg}"));
                    } else {
                        refusals.push(msg);
                    }
                }
                Err(e) => unreachable.push(e.to_string()),
            }
        }
        if refusals.is_empty() { Broadcast::Unreachable(unreachable.join("; ")) } else { Broadcast::Refused(refusals.join("; ")) }
    }

    async fn state(&self, nodes: &Nodes, a: &AttemptRow) -> Result<TxState> {
        let raw_address = a.params.get("raw_address").and_then(Value::as_str).unwrap_or(&a.from_address).to_string();
        let body_hash = a.params.get("body_hash").and_then(Value::as_str).unwrap_or_default();
        let msg_hash = a.params.get("msg_hash").and_then(Value::as_str).unwrap_or_default();
        let mut answered = false;
        for url in &nodes.urls {
            let Ok(txs) = transactions(&nodes.http, url, &raw_address, 50).await else { continue };
            answered = true;
            let found = txs.into_iter().find(|t| {
                let m = t.get("in_msg").cloned().unwrap_or(Value::Null);
                let source_empty = m.get("source").and_then(Value::as_str).unwrap_or_default().is_empty();
                source_empty
                    && (same_hash(m.get("body_hash").and_then(Value::as_str), body_hash)
                        || same_hash(m.get("hash").and_then(Value::as_str), msg_hash))
            });
            let Some(tx) = found else { continue };
            let sent: u64 =
                tx.get("out_msgs").and_then(Value::as_array).into_iter().flatten().map(|m| as_u64(m.get("value")).unwrap_or(0)).sum();
            let fee = as_u64(tx.get("fee")).unwrap_or(0);
            let success = sent > 0;
            return Ok(TxState::Included {
                // Shard blocks are final once the masterchain references them.
                confirmations: 1_000,
                success,
                fee: Some(from_base_units(fee as u128, DECIMALS)),
                error: (!success).then(|| "The wallet ran the message but sent nothing (balance or address).".to_string()),
            });
        }
        if !answered {
            return Err(Error::Unavailable(format!("no {} node answered", nodes.network.name)));
        }
        Ok(TxState::Unknown)
    }

    async fn landing(&self, nodes: &Nodes, a: &AttemptRow) -> Result<Landing> {
        let raw_address = a.params.get("raw_address").and_then(Value::as_str).unwrap_or(&a.from_address).to_string();
        let (_, seqno, _) = wallet_state(nodes, &raw_address).await?;
        let ours = a.nonce.unwrap_or(0);
        if seqno as i64 > ours {
            return Ok(Landing::NonceUsed);
        }
        Ok(match a.valid_until {
            Some(t) if Utc::now() > t + chrono::Duration::seconds(30) => Landing::Expired,
            _ => Landing::CanLand,
        })
    }

    async fn estimate_fee(&self, _nodes: &Nodes, _an: &AssetNetwork, _from: Option<&str>) -> Result<Decimal> {
        Ok(from_base_units(TYPICAL_FEE_NANOTON, DECIMALS))
    }

    async fn native_balance(&self, nodes: &Nodes, address: &str) -> Result<Decimal> {
        let v = nodes.first_ok(|u| async move { get_json(&nodes.http, &u, "/getAddressBalance", &[("address", address)]).await }).await?;
        if v.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(Error::Driver(format!("getAddressBalance: {v}")));
        }
        Ok(from_base_units(as_u64(v.get("result")).unwrap_or(0) as u128, DECIMALS))
    }

    fn lock_key(&self, chain: &str, address: &str) -> Option<String> {
        Some(format!("ton-seqno:{chain}:{address}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_wallet_address() {
        let seed = [3u8; 32];
        let a = address_from_secret(&seed, true).unwrap();
        assert_eq!(a.len(), 48);
        assert!(a.starts_with("0Q"), "testnet non-bounceable: {a}");
        assert!(address_from_secret(&seed, false).unwrap().starts_with("UQ"));
        assert_eq!(parse_address(&a).unwrap().to_hex(), wallet(&seed).unwrap().address.to_hex());
    }

    #[test]
    fn hashes_match_across_base64_alphabets() {
        let std = std_b64("90b0aa7fa6de1f0127eab48df17483cccec6d5bee0b23def73e833fa5da95120");
        assert_eq!(std, "kLCqf6beHwEn6rSN8XSDzM7G1b7gsj3vc+gz+l2pUSA=");
        assert!(same_hash(Some("kLCqf6beHwEn6rSN8XSDzM7G1b7gsj3vc-gz-l2pUSA"), &std));
        assert!(!same_hash(Some(""), ""));
        assert!(!same_hash(None, &std));
    }

    #[test]
    fn message_with_comment_builds() {
        let to = parse_address("0QAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAOk7").ok();
        let to = to.unwrap_or(TonAddress::NULL);
        assert!(internal_message(&to, 1_000_000_000, Some("order 42")).is_ok());
        assert!(internal_message(&to, 1, None).is_ok());
    }
}
