//! Tron payouts: TRX and TRC-20 tokens. The transaction is built and signed locally (protobuf by hand), so a
//! node cannot change the recipient or the amount. It expires after a few minutes; an expired transaction that
//! no node knows is signed again.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use k256::ecdsa::SigningKey;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sha3::Keccak256;
use sqlx::PgConnection;

use crate::{
    db::{AttemptRow, NewAttempt},
    domain::AssetNetwork,
    error::{Error, Result},
    payouts::{
        chain::{Broadcast, Landing, PayoutChain, SignCtx, SignMode, TxState},
        from_base_units,
        rpc::{Nodes, as_u64, post_json},
        to_base_units,
    },
};

const DECIMALS: u32 = 6;
/// Validity of a signed transaction.
const EXPIRATION_MS: i64 = 5 * 60 * 1000;
/// Upper bound of the energy fee of a TRC-20 transfer.
const FEE_LIMIT_SUN: i64 = 100_000_000;
const TRANSFER_CONTRACT: u64 = 1;
const TRIGGER_SMART_CONTRACT: u64 = 31;

pub struct Tron;

fn base58check(payload: &[u8]) -> String {
    let check = Sha256::digest(Sha256::digest(payload));
    let mut full = payload.to_vec();
    full.extend_from_slice(&check[..4]);
    bs58::encode(full).into_string()
}

/// The 21 bytes (`0x41` + 20) of a base58check T-address.
pub fn decode_address(s: &str) -> Result<[u8; 21]> {
    let bad = || Error::BadRequest(format!("`{s}` is not a Tron address."));
    let v = bs58::decode(s.trim()).into_vec().map_err(|_| bad())?;
    if v.len() != 25 || v[0] != 0x41 {
        return Err(bad());
    }
    let (payload, check) = v.split_at(21);
    if &Sha256::digest(Sha256::digest(payload))[..4] != check {
        return Err(bad());
    }
    payload.try_into().map_err(|_| bad())
}

/// T-address of a secp256k1 private key.
pub fn address_from_secret(secret: &[u8]) -> Result<String> {
    let sk = SigningKey::from_slice(secret).map_err(|_| Error::BadRequest("Not a valid secp256k1 private key.".into()))?;
    let point = sk.verifying_key().to_sec1_point(false);
    let hash = Keccak256::digest(&point.as_bytes()[1..]);
    let mut payload = vec![0x41];
    payload.extend_from_slice(&hash[12..]);
    Ok(base58check(&payload))
}

// ---- protobuf ----------------------------------------------------------------------

fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn pb_varint(out: &mut Vec<u8>, field: u64, v: u64) {
    varint(out, field << 3);
    varint(out, v);
}

fn pb_bytes(out: &mut Vec<u8>, field: u64, b: &[u8]) {
    varint(out, (field << 3) | 2);
    varint(out, b.len() as u64);
    out.extend_from_slice(b);
}

/// What the transaction does.
pub enum Call<'a> {
    Trx { owner: &'a [u8; 21], to: &'a [u8; 21], amount: i64 },
    Trc20 { owner: &'a [u8; 21], contract: &'a [u8; 21], to: &'a [u8; 21], amount: u128 },
}

/// `transfer(address,uint256)` call data.
pub fn trc20_transfer_data(to: &[u8; 21], amount: u128) -> Vec<u8> {
    let mut d = vec![0xa9, 0x05, 0x9c, 0xbb];
    d.extend_from_slice(&[0u8; 12]);
    d.extend_from_slice(&to[1..]);
    d.extend_from_slice(&[0u8; 16]);
    d.extend_from_slice(&amount.to_be_bytes());
    d
}

/// Reference block: the last two bytes of its number and bytes 8..16 of its id.
pub struct RefBlock {
    pub number: u64,
    pub id: [u8; 32],
    pub timestamp_ms: i64,
}

/// `Transaction.raw` bytes.
pub fn raw_data(call: &Call<'_>, block: &RefBlock, expiration_ms: i64, memo: Option<&str>) -> Vec<u8> {
    let (kind, type_url, param) = match call {
        Call::Trx { owner, to, amount } => {
            let mut p = Vec::new();
            pb_bytes(&mut p, 1, &owner[..]);
            pb_bytes(&mut p, 2, &to[..]);
            pb_varint(&mut p, 3, *amount as u64);
            (TRANSFER_CONTRACT, "type.googleapis.com/protocol.TransferContract", p)
        }
        Call::Trc20 { owner, contract, to, amount } => {
            let mut p = Vec::new();
            pb_bytes(&mut p, 1, &owner[..]);
            pb_bytes(&mut p, 2, &contract[..]);
            pb_bytes(&mut p, 4, &trc20_transfer_data(to, *amount));
            (TRIGGER_SMART_CONTRACT, "type.googleapis.com/protocol.TriggerSmartContract", p)
        }
    };
    let mut any = Vec::new();
    pb_bytes(&mut any, 1, type_url.as_bytes());
    pb_bytes(&mut any, 2, &param);
    let mut contract = Vec::new();
    pb_varint(&mut contract, 1, kind);
    pb_bytes(&mut contract, 2, &any);

    let mut raw = Vec::new();
    pb_bytes(&mut raw, 1, &block.number.to_be_bytes()[6..8]);
    pb_bytes(&mut raw, 4, &block.id[8..16]);
    pb_varint(&mut raw, 8, expiration_ms as u64);
    if let Some(m) = memo.filter(|m| !m.is_empty()) {
        pb_bytes(&mut raw, 10, m.as_bytes());
    }
    pb_bytes(&mut raw, 11, &contract);
    pb_varint(&mut raw, 14, block.timestamp_ms as u64);
    if matches!(call, Call::Trc20 { .. }) {
        pb_varint(&mut raw, 18, FEE_LIMIT_SUN as u64);
    }
    raw
}

/// Sign `raw` and return (txid, Transaction bytes).
pub fn sign_raw(sk: &SigningKey, raw: &[u8]) -> Result<(String, Vec<u8>)> {
    let txid = Sha256::digest(raw);
    let (sig, recid) = sk.sign_prehash_recoverable(&txid);
    let mut sig_bytes = sig.to_bytes().to_vec();
    sig_bytes.push(recid.to_byte());
    let mut tx = Vec::new();
    pb_bytes(&mut tx, 1, raw);
    pb_bytes(&mut tx, 2, &sig_bytes);
    Ok((hex::encode(txid), tx))
}

// ---- node calls ------------------------------------------------------------------------

async fn now_block(nodes: &Nodes) -> Result<RefBlock> {
    let b = nodes.first_ok(|u| async move { post_json(&nodes.http, &u, "/wallet/getnowblock", &json!({})).await }).await?;
    let number = as_u64(b.pointer("/block_header/raw_data/number")).ok_or_else(|| Error::Driver("getnowblock: no number".into()))?;
    let timestamp_ms = as_u64(b.pointer("/block_header/raw_data/timestamp")).unwrap_or(0) as i64;
    let id: [u8; 32] = b
        .get("blockID")
        .and_then(Value::as_str)
        .and_then(|s| hex::decode(s).ok())
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| Error::Driver("getnowblock: no blockID".into()))?;
    Ok(RefBlock { number, id, timestamp_ms })
}

/// (exists, balance in sun).
async fn account(nodes: &Nodes, address: &str) -> Result<(bool, u64)> {
    let body = json!({ "address": address, "visible": true });
    let v = nodes
        .first_ok(|u| {
            let body = body.clone();
            async move { post_json(&nodes.http, &u, "/wallet/getaccount", &body).await }
        })
        .await?;
    let exists = v.get("address").is_some() || v.get("create_time").is_some();
    Ok((exists, as_u64(v.get("balance")).unwrap_or(0)))
}

async fn constant_call(nodes: &Nodes, owner: &str, contract: &str, selector: &str, parameter: &str) -> Result<Value> {
    let body = json!({ "owner_address": owner, "contract_address": contract, "function_selector": selector, "parameter": parameter, "visible": true });
    nodes
        .first_ok(|u| {
            let body = body.clone();
            async move { post_json(&nodes.http, &u, "/wallet/triggerconstantcontract", &body).await }
        })
        .await
}

async fn token_balance(nodes: &Nodes, contract: &str, owner: &str, owner_bytes: &[u8; 21]) -> Result<u128> {
    let param = format!("{}{}", "0".repeat(24), hex::encode(&owner_bytes[1..]));
    let v = constant_call(nodes, owner, contract, "balanceOf(address)", &param).await?;
    let hexs = v.pointer("/constant_result/0").and_then(Value::as_str).ok_or_else(|| Error::Driver(format!("balanceOf: {v}")))?;
    let raw = hex::decode(hexs).map_err(|e| Error::Driver(format!("balanceOf: {e}")))?;
    Ok(raw.iter().skip(raw.len().saturating_sub(16)).fold(0u128, |acc, b| (acc << 8) | *b as u128))
}

/// Energy of a TRC-20 transfer, and the TRX it costs beyond the account's free energy (sun).
async fn energy_cost(nodes: &Nodes, owner: &str, contract: &str, to: &[u8; 21], amount: u128) -> Result<(u64, u64)> {
    let data = trc20_transfer_data(to, amount);
    let v = constant_call(nodes, owner, contract, "transfer(address,uint256)", &hex::encode(&data[4..])).await?;
    let energy = as_u64(v.get("energy_used")).unwrap_or(65_000);
    let params = nodes.first_ok(|u| async move { post_json(&nodes.http, &u, "/wallet/getchainparameters", &json!({})).await }).await?;
    let price = params
        .get("chainParameter")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find(|p| p.get("key").and_then(Value::as_str) == Some("getEnergyFee")))
        .and_then(|p| as_u64(p.get("value")))
        .unwrap_or(420);
    let body = json!({ "address": owner, "visible": true });
    let res = nodes
        .first_ok(|u| {
            let body = body.clone();
            async move { post_json(&nodes.http, &u, "/wallet/getaccountresource", &body).await }
        })
        .await?;
    let free = as_u64(res.get("EnergyLimit")).unwrap_or(0).saturating_sub(as_u64(res.get("EnergyUsed")).unwrap_or(0));
    Ok((energy, energy.saturating_sub(free).saturating_mul(price)))
}

fn expiration(a: &AttemptRow) -> Option<DateTime<Utc>> {
    a.valid_until
}

#[async_trait]
impl PayoutChain for Tron {
    async fn sign(&self, ctx: &SignCtx<'_>, _conn: &mut PgConnection, _mode: SignMode<'_>) -> Result<NewAttempt> {
        let nodes = ctx.nodes;
        let sk = SigningKey::from_slice(&ctx.key.secret).map_err(|_| Error::Internal("hot key is not secp256k1".into()))?;
        let from = address_from_secret(&ctx.key.secret)?;
        if from != ctx.key.address {
            return Err(Error::Internal("the hot key does not match its address".into()));
        }
        let owner = decode_address(&from)?;
        let to = decode_address(&ctx.payout.address)?;
        let raw_amount = to_base_units(ctx.payout.amount, ctx.an.decimals as u32)?;
        let (_, trx) = account(nodes, &from).await?;
        let contract_bytes = match &ctx.an.contract_address {
            Some(c) => decode_address(c).map_err(|_| Error::Unavailable(format!("The token contract {c} is not a Tron address.")))?,
            None => [0u8; 21],
        };
        let call = match &ctx.an.contract_address {
            None => {
                let amount = i64::try_from(raw_amount).map_err(|_| Error::BadRequest("The amount is too large.".into()))?;
                let (to_exists, _) = account(nodes, &ctx.payout.address).await?;
                // A new account costs about 1.1 TRX to activate; bandwidth costs about 0.3 TRX.
                let reserve: u128 = if to_exists { 300_000 } else { 1_400_000 };
                let need = raw_amount + reserve;
                if (trx as u128) < need {
                    return Err(Error::BadRequest(format!(
                        "The hot wallet holds {} TRX. This payout needs about {} TRX with the network fee.",
                        from_base_units(trx as u128, DECIMALS),
                        from_base_units(need, DECIMALS)
                    )));
                }
                Call::Trx { owner: &owner, to: &to, amount }
            }
            Some(contract) => {
                let have = token_balance(nodes, contract, &from, &owner).await?;
                if have < raw_amount {
                    return Err(Error::BadRequest(format!(
                        "The hot wallet holds {} {}. This payout needs {}.",
                        from_base_units(have, ctx.an.decimals as u32),
                        ctx.an.asset.symbol,
                        ctx.payout.amount.normalize()
                    )));
                }
                let (_, energy_sun) = energy_cost(nodes, &from, contract, &to, raw_amount).await?;
                let need = energy_sun as u128 + 400_000;
                if (trx as u128) < need {
                    return Err(Error::BadRequest(format!(
                        "The hot wallet holds {} TRX. The energy and bandwidth of this payout cost about {} TRX.",
                        from_base_units(trx as u128, DECIMALS),
                        from_base_units(need, DECIMALS)
                    )));
                }
                Call::Trc20 { owner: &owner, contract: &contract_bytes, to: &to, amount: raw_amount }
            }
        };
        let block = now_block(nodes).await?;
        let base_ms = block.timestamp_ms.max(Utc::now().timestamp_millis());
        let expiration_ms = base_ms + EXPIRATION_MS;
        let raw = raw_data(&call, &block, expiration_ms, ctx.payout.extra_id.as_deref());
        let (txid, tx) = sign_raw(&sk, &raw)?;
        Ok(NewAttempt {
            from_address: from,
            txid,
            raw_tx: hex::encode(tx),
            nonce: None,
            params: json!({ "ref_block": block.number, "expiration_ms": expiration_ms }),
            valid_until: DateTime::<Utc>::from_timestamp_millis(expiration_ms),
        })
    }

    async fn broadcast(&self, nodes: &Nodes, a: &AttemptRow) -> Broadcast {
        let mut refusals = Vec::new();
        let mut unreachable = Vec::new();
        for url in &nodes.urls {
            match post_json(&nodes.http, url, "/wallet/broadcasthex", &json!({ "transaction": a.raw_tx })).await {
                Ok(v) if v.get("result").and_then(Value::as_bool) == Some(true) => return Broadcast::Accepted,
                Ok(v) => {
                    let code = v.get("code").and_then(Value::as_str).unwrap_or("");
                    if code == "DUP_TRANSACTION_ERROR" {
                        return Broadcast::Accepted;
                    }
                    let msg = v
                        .get("message")
                        .and_then(Value::as_str)
                        .map(|m| hex::decode(m).ok().and_then(|b| String::from_utf8(b).ok()).unwrap_or_else(|| m.to_string()))
                        .unwrap_or_default();
                    if matches!(code, "SERVER_BUSY" | "NO_CONNECTION" | "NOT_ENOUGH_EFFECTIVE_CONNECTION" | "BLOCK_UNSOLIDIFIED") {
                        unreachable.push(format!("{code} {msg}"));
                    } else {
                        refusals.push(format!("{code} {msg}").trim().to_string());
                    }
                }
                Err(e) => unreachable.push(e.to_string()),
            }
        }
        if refusals.is_empty() { Broadcast::Unreachable(unreachable.join("; ")) } else { Broadcast::Refused(refusals.join("; ")) }
    }

    async fn state(&self, nodes: &Nodes, a: &AttemptRow) -> Result<TxState> {
        let mut answered = false;
        let mut pending = false;
        let q = json!({ "value": a.txid });
        for url in &nodes.urls {
            let Ok(info) = post_json(&nodes.http, url, "/wallet/gettransactioninfobyid", &q).await else { continue };
            answered = true;
            if let Some(block) = as_u64(info.get("blockNumber")) {
                let head = post_json(&nodes.http, url, "/wallet/getnowblock", &json!({}))
                    .await
                    .ok()
                    .and_then(|b| as_u64(b.pointer("/block_header/raw_data/number")))
                    .unwrap_or(block);
                let receipt_result = info.pointer("/receipt/result").and_then(Value::as_str);
                let failed = info.get("result").and_then(Value::as_str) == Some("FAILED") || receipt_result.is_some_and(|r| r != "SUCCESS");
                let msg = info
                    .get("resMessage")
                    .and_then(Value::as_str)
                    .and_then(|m| hex::decode(m).ok())
                    .and_then(|b| String::from_utf8(b).ok())
                    .or_else(|| receipt_result.map(str::to_string))
                    .unwrap_or_default();
                return Ok(TxState::Included {
                    confirmations: head.saturating_sub(block) as i64 + 1,
                    success: !failed,
                    fee: Some(from_base_units(as_u64(info.get("fee")).unwrap_or(0) as u128, DECIMALS)),
                    error: failed.then(|| format!("The transaction failed on the chain: {msg}. The network fee was spent.")),
                });
            }
            for path in ["/wallet/gettransactionbyid", "/wallet/gettransactionfrompending"] {
                if let Ok(tx) = post_json(&nodes.http, url, path, &q).await
                    && tx.get("txID").is_some()
                {
                    pending = true;
                }
            }
        }
        if !answered {
            return Err(Error::Unavailable(format!("no {} node answered", nodes.network.name)));
        }
        Ok(if pending { TxState::Pending } else { TxState::Unknown })
    }

    async fn landing(&self, _nodes: &Nodes, a: &AttemptRow) -> Result<Landing> {
        // Nodes drop a transaction at its expiration; allow for clock drift.
        Ok(match expiration(a) {
            Some(t) if Utc::now() > t + chrono::Duration::seconds(60) => Landing::Expired,
            _ => Landing::CanLand,
        })
    }

    async fn estimate_fee(&self, nodes: &Nodes, an: &AssetNetwork, from: Option<&str>) -> Result<Decimal> {
        match (&an.contract_address, from) {
            (None, _) => Ok(from_base_units(300_000, DECIMALS)),
            (Some(contract), Some(from)) => {
                let owner = decode_address(from)?;
                let (_, sun) = energy_cost(nodes, from, contract, &owner, 1).await?;
                Ok(from_base_units(sun as u128 + 350_000, DECIMALS))
            }
            (Some(_), None) => Ok(from_base_units(30_000_000, DECIMALS)),
        }
    }

    async fn native_balance(&self, nodes: &Nodes, address: &str) -> Result<Decimal> {
        Ok(from_base_units(account(nodes, address).await?.1 as u128, DECIMALS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_of_known_key() {
        let key = hex::decode("ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80").unwrap();
        let a = address_from_secret(&key).unwrap();
        assert!(a.starts_with('T') && a.len() == 34, "{a}");
        // The same 20 bytes as the EVM address of the key.
        assert_eq!(hex::encode(&decode_address(&a).unwrap()[1..]), "f39fd6e51aad88f6f4ce6ab8827279cfffb92266");
        assert!(decode_address("TQrZ9wBzhXnHdAdKwRBfdMH2RiTjH6yGtX").is_err());
    }

    #[test]
    fn transfer_contract_encoding() {
        let owner = [0x41u8; 21];
        let to = [0x42u8; 21];
        let block = RefBlock { number: 0x0102_0304, id: [7u8; 32], timestamp_ms: 1_000 };
        let raw = raw_data(&Call::Trx { owner: &owner, to: &to, amount: 5 }, &block, 61_000, None);
        // ref_block_bytes = last two bytes of the number; ref_block_hash = id[8..16].
        assert_eq!(&raw[..4], &[0x0a, 2, 0x03, 0x04]);
        assert_eq!(&raw[4..6], &[0x22, 8]);
        assert_eq!(&raw[6..14], &[7u8; 8]);
        let s = hex::encode(&raw);
        assert!(s.contains(&hex::encode(b"type.googleapis.com/protocol.TransferContract")));
        // amount field (3) of the contract = 5
        assert!(s.contains("1805"));
    }

    #[test]
    fn trc20_call_data() {
        let key = hex::decode("ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80").unwrap();
        let to = decode_address(&address_from_secret(&key).unwrap()).unwrap();
        let d = trc20_transfer_data(&to, 250_000_000);
        assert_eq!(d.len(), 68);
        assert_eq!(&d[..4], &[0xa9, 0x05, 0x9c, 0xbb]);
        assert_eq!(&d[16..36], &to[1..]);
        assert_eq!(u128::from_be_bytes(d[52..68].try_into().unwrap()), 250_000_000);
    }
}
