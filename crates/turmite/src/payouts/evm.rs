//! EVM payouts: the native coin and ERC-20 tokens, signed locally. EIP-1559 transactions, or legacy ones on a
//! chain without a base fee. A stuck transaction is signed again with the same nonce, recipient and amount and
//! higher fees.

use std::str::FromStr;

use alloy::consensus::{SignableTransaction, TxEip1559, TxEnvelope, TxLegacy};
use alloy::eips::eip2718::Encodable2718;
use alloy::primitives::{Address, Bytes, TxKind, U256};
use alloy::signers::{SignerSync, local::PrivateKeySigner};
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::PgConnection;

use crate::{
    db::{AttemptRow, NewAttempt, PgStore},
    domain::AssetNetwork,
    error::{Error, Result},
    payouts::{
        chain::{Broadcast, Landing, PayoutChain, SignCtx, SignMode, TxState},
        from_base_units,
        rpc::{Nodes, hex_u64, hex_u128, json_rpc},
        to_base_units,
    },
};

const NATIVE_DECIMALS: u32 = 18;
const ONE_GWEI: u128 = 1_000_000_000;
/// A transaction that waits this long in the mempool gets higher fees.
const BUMP_AFTER_SECS: i64 = 180;

alloy::sol! {
    #[allow(missing_docs)]
    interface IErc20Payout {
        function balanceOf(address owner) external view returns (uint256);
        function transfer(address to, uint256 amount) external returns (bool);
    }
}

pub struct Evm;

/// Parse an EVM address. A mixed-case address must carry a valid EIP-55 checksum. Returns the checksummed form.
pub fn normalize_address(s: &str) -> Result<String> {
    let s = s.trim();
    let bad = || Error::BadRequest(format!("`{s}` is not an EVM address."));
    let hex_part = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).ok_or_else(bad)?;
    if hex_part.len() != 40 || !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(bad());
    }
    let has_lower = hex_part.chars().any(|c| c.is_ascii_lowercase());
    let has_upper = hex_part.chars().any(|c| c.is_ascii_uppercase());
    let addr = if has_lower && has_upper {
        Address::parse_checksummed(format!("0x{hex_part}"), None)
            .map_err(|_| Error::BadRequest(format!("`{s}` has a wrong EIP-55 checksum. Check the address.")))?
    } else {
        Address::from_str(&format!("0x{hex_part}")).map_err(|_| bad())?
    };
    Ok(addr.to_checksum(None))
}

/// EIP-55 address of a secp256k1 private key.
pub fn address_from_secret(secret: &[u8]) -> Result<String> {
    let signer = PrivateKeySigner::from_slice(secret).map_err(|_| Error::BadRequest("Not a valid secp256k1 private key.".into()))?;
    Ok(signer.address().to_checksum(None))
}

fn addr(s: &str) -> Result<Address> {
    Address::from_str(s.trim()).map_err(|_| Error::BadRequest(format!("`{s}` is not an EVM address.")))
}

fn hex_quantity(v: u128) -> String {
    format!("{v:#x}")
}

fn transfer_data(to: Address, raw: u128) -> Bytes {
    IErc20Payout::transferCall { to, amount: U256::from(raw) }.abi_encode().into()
}

/// Fees for a new transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fees {
    Eip1559 { max_fee: u128, priority: u128 },
    Legacy { gas_price: u128 },
}

impl Fees {
    pub fn max_per_gas(self) -> u128 {
        match self {
            Fees::Eip1559 { max_fee, .. } => max_fee,
            Fees::Legacy { gas_price } => gas_price,
        }
    }

    fn to_json(self, gas_limit: u64) -> Value {
        match self {
            Fees::Eip1559 { max_fee, priority } => {
                json!({ "kind": "eip1559", "max_fee": max_fee.to_string(), "priority": priority.to_string(), "gas_limit": gas_limit })
            }
            Fees::Legacy { gas_price } => json!({ "kind": "legacy", "gas_price": gas_price.to_string(), "gas_limit": gas_limit }),
        }
    }

    fn from_json(v: &Value) -> Option<(Self, u64)> {
        let n = |k: &str| v.get(k).and_then(Value::as_str).and_then(|s| s.parse::<u128>().ok());
        let gas_limit = v.get("gas_limit").and_then(Value::as_u64)?;
        match v.get("kind").and_then(Value::as_str)? {
            "eip1559" => Some((Fees::Eip1559 { max_fee: n("max_fee")?, priority: n("priority")? }, gas_limit)),
            "legacy" => Some((Fees::Legacy { gas_price: n("gas_price")? }, gas_limit)),
            _ => None,
        }
    }

    /// Fees for a replacement: at least 25 % more than `old` (nodes ask for 10 %) and not below `current`.
    pub fn bumped(old: Fees, current: Fees) -> Fees {
        let up = |x: u128| x.saturating_mul(5).div_ceil(4).max(x + 1);
        match (old, current) {
            (Fees::Eip1559 { max_fee, priority }, Fees::Eip1559 { max_fee: cur_max, priority: cur_prio }) => {
                let priority = up(priority).max(cur_prio);
                Fees::Eip1559 { max_fee: up(max_fee).max(cur_max).max(priority), priority }
            }
            (Fees::Legacy { gas_price }, cur) => Fees::Legacy { gas_price: up(gas_price).max(cur.max_per_gas()) },
            (Fees::Eip1559 { max_fee, priority }, Fees::Legacy { gas_price }) => {
                let priority = up(priority);
                Fees::Eip1559 { max_fee: up(max_fee).max(gas_price).max(priority), priority }
            }
        }
    }
}

async fn current_fees(nodes: &Nodes) -> Result<Fees> {
    let block = nodes.rpc("eth_getBlockByNumber", json!(["latest", false])).await?;
    match block.get("baseFeePerGas").and_then(hex_u128) {
        Some(base) => {
            let suggested = nodes.rpc("eth_maxPriorityFeePerGas", json!([])).await.ok().and_then(|v| hex_u128(&v));
            let priority = suggested.unwrap_or(ONE_GWEI);
            Ok(Fees::Eip1559 { max_fee: base.saturating_mul(2).saturating_add(priority).max(1), priority })
        }
        None => {
            let gas_price =
                hex_u128(&nodes.rpc("eth_gasPrice", json!([])).await?).ok_or_else(|| Error::Driver("eth_gasPrice: bad answer".into()))?;
            Ok(Fees::Legacy { gas_price })
        }
    }
}

async fn balance_wei(nodes: &Nodes, address: &str, tag: &str) -> Result<u128> {
    let v = nodes.rpc("eth_getBalance", json!([address, tag])).await?;
    hex_u128(&v).ok_or_else(|| Error::Driver("eth_getBalance: bad answer".into()))
}

async fn token_balance(nodes: &Nodes, token: &str, owner: Address) -> Result<u128> {
    let data = IErc20Payout::balanceOfCall { owner }.abi_encode();
    let v = nodes.rpc("eth_call", json!([{ "to": token, "data": format!("0x{}", hex::encode(data)) }, "latest"])).await?;
    let raw = hex::decode(v.as_str().unwrap_or("").trim_start_matches("0x")).map_err(|e| Error::Driver(format!("balanceOf: {e}")))?;
    if raw.len() < 32 {
        return Err(Error::Driver("balanceOf: short answer".into()));
    }
    u128::try_from(U256::from_be_slice(&raw[..32])).map_err(|_| Error::Driver("balanceOf: too large".into()))
}

/// `eth_estimateGas`. A revert is a refusal by the chain, not a node problem.
async fn estimate_gas(nodes: &Nodes, call: Value) -> Result<u64> {
    match nodes.rpc("eth_estimateGas", json!([call])).await {
        Ok(v) => hex_u64(&v).ok_or_else(|| Error::Driver("eth_estimateGas: bad answer".into())),
        Err(Error::Driver(msg)) if msg.contains("revert") || msg.contains("insufficient funds") => {
            Err(Error::BadRequest(format!("The chain refuses this transfer: {msg}")))
        }
        Err(e) => Err(e),
    }
}

/// Signed transaction bytes and hash.
#[allow(clippy::too_many_arguments)]
fn sign_tx(
    signer: &PrivateKeySigner,
    chain_id: u64,
    nonce: u64,
    gas_limit: u64,
    fees: Fees,
    to: Address,
    value: U256,
    input: Bytes,
) -> Result<(String, String)> {
    let envelope = match fees {
        Fees::Eip1559 { max_fee, priority } => {
            let tx = TxEip1559 {
                chain_id,
                nonce,
                gas_limit,
                max_fee_per_gas: max_fee,
                max_priority_fee_per_gas: priority,
                to: TxKind::Call(to),
                value,
                access_list: Default::default(),
                input,
            };
            let sig = signer.sign_hash_sync(&tx.signature_hash()).map_err(|e| Error::Internal(format!("evm sign: {e}")))?;
            TxEnvelope::Eip1559(tx.into_signed(sig))
        }
        Fees::Legacy { gas_price } => {
            let tx = TxLegacy { chain_id: Some(chain_id), nonce, gas_price, gas_limit, to: TxKind::Call(to), value, input };
            let sig = signer.sign_hash_sync(&tx.signature_hash()).map_err(|e| Error::Internal(format!("evm sign: {e}")))?;
            TxEnvelope::Legacy(tx.into_signed(sig))
        }
    };
    let hash = format!("{:#x}", envelope.tx_hash());
    Ok((hash, format!("0x{}", hex::encode(envelope.encoded_2718()))))
}

#[async_trait]
impl PayoutChain for Evm {
    async fn sign(&self, ctx: &SignCtx<'_>, conn: &mut PgConnection, mode: SignMode<'_>) -> Result<NewAttempt> {
        let network = &ctx.nodes.network;
        let from = ctx.key.address.clone();
        // The same lock as EVM sweeps and deploys: one nonce reader per wallet and chain.
        PgStore::lock_evm_nonce(conn, &network.chain, &from).await?;
        let signer = PrivateKeySigner::from_slice(&ctx.key.secret).map_err(|_| Error::Internal("hot key is not secp256k1".into()))?;
        if !signer.address().to_checksum(None).eq_ignore_ascii_case(&from) {
            return Err(Error::Internal("the hot key does not match its address".into()));
        }
        let chain_id =
            hex_u64(&ctx.nodes.rpc("eth_chainId", json!([])).await?).ok_or_else(|| Error::Driver("eth_chainId: bad answer".into()))?;
        if let Some(want) = network.chain_id
            && want as u64 != chain_id
        {
            return Err(Error::Unavailable(format!("The {} node is on chain {chain_id}, not {want}.", network.name)));
        }
        let to = addr(&ctx.payout.address)?;
        let raw = to_base_units(ctx.payout.amount, ctx.an.decimals as u32)?;
        let (call_to, value, input) = match &ctx.an.contract_address {
            None => (to, U256::from(raw), Bytes::new()),
            Some(token) => (addr(token)?, U256::ZERO, transfer_data(to, raw)),
        };
        let current = current_fees(ctx.nodes).await?;

        let (nonce, fees, gas_limit) = match mode {
            SignMode::Bump(prev) => {
                let (old, gas_limit) = Fees::from_json(&prev.params).ok_or_else(|| Error::Internal("stored fee parameters".into()))?;
                let nonce = prev.nonce.ok_or_else(|| Error::Internal("stored nonce".into()))? as u64;
                (nonce, Fees::bumped(old, current), gas_limit)
            }
            SignMode::New | SignMode::Renew => {
                let pending = hex_u64(&ctx.nodes.rpc("eth_getTransactionCount", json!([from, "pending"])).await?)
                    .ok_or_else(|| Error::Driver("eth_getTransactionCount: bad answer".into()))?;
                let nonce = PgStore::next_evm_nonce(conn, network.network_id, &from, pending).await?;
                // Check the token balance first: a gas estimate of a transfer without funds only says "revert".
                if let Some(token) = &ctx.an.contract_address {
                    let have = token_balance(ctx.nodes, token, addr(&from)?).await?;
                    if have < raw {
                        return Err(Error::BadRequest(format!(
                            "The hot wallet holds {} {}. This payout needs {}.",
                            from_base_units(have, ctx.an.decimals as u32),
                            ctx.an.asset.symbol,
                            ctx.payout.amount.normalize()
                        )));
                    }
                }
                let call = match &ctx.an.contract_address {
                    None => json!({ "from": from, "to": format!("{to:#x}"), "value": hex_quantity(raw) }),
                    Some(token) => json!({ "from": from, "to": token, "data": format!("0x{}", hex::encode(&input)) }),
                };
                let est = estimate_gas(ctx.nodes, call).await?;
                let gas_limit = match &ctx.an.contract_address {
                    None if est <= 21_000 => 21_000,
                    None => est.saturating_mul(12) / 10,
                    Some(_) => (est.saturating_mul(13) / 10).max(50_000),
                };
                (nonce, current, gas_limit)
            }
        };

        // The hot wallet must hold the amount and the largest possible fee.
        let max_gas_cost = (gas_limit as u128).saturating_mul(fees.max_per_gas());
        let native = balance_wei(ctx.nodes, &from, "latest").await?;
        let symbol = &network.code;
        match &ctx.an.contract_address {
            None => {
                let need = raw.saturating_add(max_gas_cost);
                if native < need && !matches!(mode, SignMode::Bump(_)) {
                    return Err(Error::BadRequest(format!(
                        "The hot wallet holds {} {symbol} native coin. This payout needs up to {} with the network fee.",
                        from_base_units(native, NATIVE_DECIMALS),
                        from_base_units(need, NATIVE_DECIMALS)
                    )));
                }
            }
            Some(_) => {
                if native < max_gas_cost && !matches!(mode, SignMode::Bump(_)) {
                    return Err(Error::BadRequest(format!(
                        "The hot wallet holds {} native coin on {}. The network fee can be up to {}.",
                        from_base_units(native, NATIVE_DECIMALS),
                        network.name,
                        from_base_units(max_gas_cost, NATIVE_DECIMALS)
                    )));
                }
            }
        }

        let (txid, raw_tx) = sign_tx(&signer, chain_id, nonce, gas_limit, fees, call_to, value, input)?;
        Ok(NewAttempt { from_address: from, txid, raw_tx, nonce: Some(nonce as i64), params: fees.to_json(gas_limit), valid_until: None })
    }

    async fn broadcast(&self, nodes: &Nodes, a: &AttemptRow) -> Broadcast {
        let mut refusals = Vec::new();
        let mut unreachable = Vec::new();
        for url in &nodes.urls {
            match json_rpc(&nodes.http, url, "eth_sendRawTransaction", json!([a.raw_tx])).await {
                Ok(_) => return Broadcast::Accepted,
                Err(Error::Driver(msg)) => {
                    let lower = msg.to_ascii_lowercase();
                    if lower.contains("already known") || lower.contains("known transaction") || lower.contains("alreadyknown") {
                        return Broadcast::Accepted;
                    }
                    refusals.push(msg);
                }
                Err(e) => unreachable.push(e.to_string()),
            }
        }
        if refusals.is_empty() { Broadcast::Unreachable(unreachable.join("; ")) } else { Broadcast::Refused(refusals.join("; ")) }
    }

    async fn state(&self, nodes: &Nodes, a: &AttemptRow) -> Result<TxState> {
        let mut answered = false;
        let mut pending = false;
        for url in &nodes.urls {
            let receipt = match json_rpc(&nodes.http, url, "eth_getTransactionReceipt", json!([a.txid])).await {
                Ok(r) => r,
                Err(_) => continue,
            };
            answered = true;
            if !receipt.is_null() {
                let block = receipt.get("blockNumber").and_then(hex_u64).unwrap_or(0);
                let head = json_rpc(&nodes.http, url, "eth_blockNumber", json!([])).await.ok().and_then(|v| hex_u64(&v)).unwrap_or(block);
                let success = receipt.get("status").and_then(hex_u64) == Some(1);
                let gas_used = receipt.get("gasUsed").and_then(hex_u128).unwrap_or(0);
                let price = receipt.get("effectiveGasPrice").and_then(hex_u128).unwrap_or(0);
                return Ok(TxState::Included {
                    confirmations: head.saturating_sub(block) as i64 + 1,
                    success,
                    fee: Some(from_base_units(gas_used.saturating_mul(price), NATIVE_DECIMALS)),
                    error: (!success).then(|| "The transaction reverted on the chain. The network fee was spent.".to_string()),
                });
            }
            if let Ok(tx) = json_rpc(&nodes.http, url, "eth_getTransactionByHash", json!([a.txid])).await
                && !tx.is_null()
            {
                pending = true;
            }
        }
        if !answered {
            return Err(Error::Unavailable(format!("no {} node answered", nodes.network.name)));
        }
        Ok(if pending { TxState::Pending } else { TxState::Unknown })
    }

    async fn landing(&self, nodes: &Nodes, a: &AttemptRow) -> Result<Landing> {
        let Some(nonce) = a.nonce else { return Ok(Landing::CanLand) };
        let latest = hex_u64(&nodes.rpc("eth_getTransactionCount", json!([a.from_address, "latest"])).await?)
            .ok_or_else(|| Error::Driver("eth_getTransactionCount: bad answer".into()))?;
        Ok(if latest > nonce as u64 { Landing::NonceUsed } else { Landing::CanLand })
    }

    async fn estimate_fee(&self, nodes: &Nodes, an: &AssetNetwork, from: Option<&str>) -> Result<Decimal> {
        let fees = current_fees(nodes).await?;
        let price = match fees {
            Fees::Eip1559 { max_fee, priority } => (max_fee - priority) / 2 + priority,
            Fees::Legacy { gas_price } => gas_price,
        };
        let gas = match (&an.contract_address, from) {
            (None, _) => 21_000,
            (Some(token), Some(from)) => {
                // Estimate a transfer of one base unit to the sender itself.
                let data = transfer_data(addr(from)?, 1);
                let call = json!({ "from": from, "to": token, "data": format!("0x{}", hex::encode(&data)) });
                estimate_gas(nodes, call).await.unwrap_or(65_000)
            }
            (Some(_), None) => 65_000,
        };
        Ok(from_base_units((gas as u128).saturating_mul(price), NATIVE_DECIMALS))
    }

    async fn native_balance(&self, nodes: &Nodes, address: &str) -> Result<Decimal> {
        Ok(from_base_units(balance_wei(nodes, address, "latest").await?, NATIVE_DECIMALS))
    }

    fn bump_after_secs(&self) -> Option<i64> {
        Some(BUMP_AFTER_SECS)
    }

    fn lock_key(&self, chain: &str, address: &str) -> Option<String> {
        Some(format!("evm-nonce:{chain}:{}", address.to_lowercase()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_rules() {
        let good = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";
        assert_eq!(normalize_address(good).unwrap(), good);
        assert_eq!(normalize_address(&good.to_lowercase()).unwrap(), good);
        assert_eq!(normalize_address(&format!("0x{}", good[2..].to_uppercase())).unwrap(), good);
        // One letter in the wrong case.
        assert!(normalize_address("0xF39Fd6e51aad88F6F4ce6aB8827279cffFb92266").is_err());
        assert!(normalize_address("0x1234").is_err());
        assert!(normalize_address("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266").is_err());
    }

    #[test]
    fn address_of_known_key() {
        let key = hex::decode("ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80").unwrap();
        assert_eq!(address_from_secret(&key).unwrap(), "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
    }

    #[test]
    fn fee_bump_is_at_least_a_quarter() {
        let old = Fees::Eip1559 { max_fee: 100, priority: 10 };
        let low = Fees::Eip1559 { max_fee: 50, priority: 5 };
        assert_eq!(Fees::bumped(old, low), Fees::Eip1559 { max_fee: 125, priority: 13 });
        let high = Fees::Eip1559 { max_fee: 400, priority: 40 };
        assert_eq!(Fees::bumped(old, high), Fees::Eip1559 { max_fee: 400, priority: 40 });
        assert_eq!(Fees::bumped(Fees::Legacy { gas_price: 8 }, Fees::Legacy { gas_price: 1 }), Fees::Legacy { gas_price: 10 });
        let (f, g) = Fees::from_json(&old.to_json(21_000)).unwrap();
        assert_eq!((f, g), (old, 21_000));
    }

    #[test]
    fn signs_a_valid_transaction() {
        let key = hex::decode("ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80").unwrap();
        let signer = PrivateKeySigner::from_slice(&key).unwrap();
        let to = addr("0x70997970C51812dc3A010C7d01b50e0d17dc79C8").unwrap();
        let (hash, raw) = sign_tx(
            &signer,
            31337,
            0,
            21_000,
            Fees::Eip1559 { max_fee: 2 * ONE_GWEI, priority: ONE_GWEI },
            to,
            U256::from(1u64),
            Bytes::new(),
        )
        .unwrap();
        assert!(hash.starts_with("0x") && hash.len() == 66);
        assert!(raw.starts_with("0x02"), "type-2 envelope: {raw}");
        let bytes = hex::decode(&raw[2..]).unwrap();
        assert_eq!(format!("{:#x}", alloy::primitives::keccak256(&bytes)), hash);
    }
}
