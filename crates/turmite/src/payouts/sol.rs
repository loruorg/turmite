//! Solana payouts: SOL and SPL tokens (Token and Token-2022), signed locally as legacy transactions.
//!
//! A token payout goes to the recipient's associated token account. The hot wallet creates that account when it
//! is missing and pays its rent. A token account is refused as the recipient: it must be a wallet address.
//! A SOL payout that would leave a new account below the rent-exempt minimum is refused: the chain would reject it.

use async_trait::async_trait;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;

use crate::{
    db::{AttemptRow, NewAttempt},
    domain::AssetNetwork,
    error::{Error, Result},
    payouts::{
        chain::{Broadcast, Landing, PayoutChain, SignCtx, SignMode, TxState},
        from_base_units,
        rpc::{Nodes, as_u64, json_rpc},
        to_base_units,
    },
};

pub type Pubkey = [u8; 32];

const DECIMALS: u32 = 9;
pub const SYSTEM_PROGRAM: Pubkey = [0u8; 32];
pub const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
pub const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
pub const ATA_PROGRAM: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
/// Size of a token account without extensions; Token-2022 associated accounts add the immutable-owner extension.
const TOKEN_ACCOUNT_LEN: u64 = 165;
const TOKEN_2022_ACCOUNT_LEN: u64 = 170;

pub struct Sol;

pub fn decode_pubkey(s: &str) -> Result<Pubkey> {
    let v = bs58::decode(s.trim()).into_vec().map_err(|_| Error::BadRequest(format!("`{s}` is not a Solana address.")))?;
    v.try_into().map_err(|_| Error::BadRequest(format!("`{s}` is not a Solana address.")))
}

pub fn encode_pubkey(k: &Pubkey) -> String {
    bs58::encode(k).into_string()
}

fn program(s: &str) -> Pubkey {
    decode_pubkey(s).expect("program ids are valid base58")
}

/// Address of an ed25519 seed (the 32-byte secret of a hot wallet).
pub fn address_from_secret(secret: &[u8]) -> Result<String> {
    let seed: [u8; 32] = secret.try_into().map_err(|_| Error::BadRequest("A Solana key has a 32-byte seed.".into()))?;
    Ok(encode_pubkey(&SigningKey::from_bytes(&seed).verifying_key().to_bytes()))
}

fn on_curve(k: &Pubkey) -> bool {
    VerifyingKey::from_bytes(k).is_ok()
}

/// `find_program_address`: the first bump from 255 down that gives an address off the ed25519 curve.
pub fn find_program_address(seeds: &[&[u8]], program_id: &Pubkey) -> Option<(Pubkey, u8)> {
    for bump in (0..=255u8).rev() {
        let mut h = Sha256::new();
        for s in seeds {
            h.update(s);
        }
        h.update([bump]);
        h.update(program_id);
        h.update(b"ProgramDerivedAddress");
        let out: Pubkey = h.finalize().into();
        if !on_curve(&out) {
            return Some((out, bump));
        }
    }
    None
}

/// Associated token account of `owner` for `mint`.
pub fn associated_token_address(owner: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    find_program_address(&[owner, token_program, mint], &program(ATA_PROGRAM)).map(|(k, _)| k).expect("a bump exists")
}

#[derive(Clone, Debug)]
pub struct AccountMeta {
    pub pubkey: Pubkey,
    pub is_signer: bool,
    pub is_writable: bool,
}

impl AccountMeta {
    pub fn new(pubkey: Pubkey, is_signer: bool, is_writable: bool) -> Self {
        Self { pubkey, is_signer, is_writable }
    }
}

#[derive(Clone, Debug)]
pub struct Instruction {
    pub program_id: Pubkey,
    pub accounts: Vec<AccountMeta>,
    pub data: Vec<u8>,
}

fn compact_u16(out: &mut Vec<u8>, mut n: usize) {
    loop {
        let mut b = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(b);
            return;
        }
        b |= 0x80;
        out.push(b);
    }
}

/// A legacy message. Accounts: the fee payer first, then writable signers, read-only signers, writable and
/// read-only non-signers, each group in first-use order.
pub fn compile_message(payer: &Pubkey, instructions: &[Instruction], blockhash: &[u8; 32]) -> (Vec<u8>, usize) {
    let mut metas: Vec<AccountMeta> = vec![AccountMeta::new(*payer, true, true)];
    let mut add = |m: AccountMeta| match metas.iter_mut().find(|x| x.pubkey == m.pubkey) {
        Some(x) => {
            x.is_signer |= m.is_signer;
            x.is_writable |= m.is_writable;
        }
        None => metas.push(m),
    };
    for ix in instructions {
        for a in &ix.accounts {
            add(a.clone());
        }
        add(AccountMeta::new(ix.program_id, false, false));
    }
    let group = |m: &AccountMeta| match (m.is_signer, m.is_writable) {
        (true, true) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (false, false) => 3,
    };
    let payer_meta = metas.remove(0);
    let mut ordered = vec![payer_meta];
    for g in 0..4 {
        ordered.extend(metas.iter().filter(|m| group(m) == g).cloned());
    }
    let signers = ordered.iter().filter(|m| m.is_signer).count();
    let readonly_signed = ordered.iter().filter(|m| m.is_signer && !m.is_writable).count();
    let readonly_unsigned = ordered.iter().filter(|m| !m.is_signer && !m.is_writable).count();
    let index = |k: &Pubkey| ordered.iter().position(|m| &m.pubkey == k).expect("account is listed") as u8;

    let mut m = vec![signers as u8, readonly_signed as u8, readonly_unsigned as u8];
    compact_u16(&mut m, ordered.len());
    for a in &ordered {
        m.extend_from_slice(&a.pubkey);
    }
    m.extend_from_slice(blockhash);
    compact_u16(&mut m, instructions.len());
    for ix in instructions {
        m.push(index(&ix.program_id));
        compact_u16(&mut m, ix.accounts.len());
        for a in &ix.accounts {
            m.push(index(&a.pubkey));
        }
        compact_u16(&mut m, ix.data.len());
        m.extend_from_slice(&ix.data);
    }
    (m, signers)
}

/// Sign a message with every signer (in account order) and serialize the transaction.
pub fn sign_transaction(message: &[u8], signers: &[&SigningKey]) -> (Vec<u8>, String) {
    let mut tx = Vec::with_capacity(1 + 64 * signers.len() + message.len());
    compact_u16(&mut tx, signers.len());
    let mut first = String::new();
    for (i, s) in signers.iter().enumerate() {
        let sig = s.sign(message).to_bytes();
        if i == 0 {
            first = bs58::encode(sig).into_string();
        }
        tx.extend_from_slice(&sig);
    }
    tx.extend_from_slice(message);
    (tx, first)
}

pub fn system_transfer(from: &Pubkey, to: &Pubkey, lamports: u64) -> Instruction {
    let mut data = vec![2, 0, 0, 0];
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction {
        program_id: SYSTEM_PROGRAM,
        accounts: vec![AccountMeta::new(*from, true, true), AccountMeta::new(*to, false, true)],
        data,
    }
}

/// `CreateIdempotent` of the associated token account program.
pub fn create_ata_idempotent(payer: &Pubkey, ata: &Pubkey, owner: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Instruction {
    Instruction {
        program_id: program(ATA_PROGRAM),
        accounts: vec![
            AccountMeta::new(*payer, true, true),
            AccountMeta::new(*ata, false, true),
            AccountMeta::new(*owner, false, false),
            AccountMeta::new(*mint, false, false),
            AccountMeta::new(SYSTEM_PROGRAM, false, false),
            AccountMeta::new(*token_program, false, false),
        ],
        data: vec![1],
    }
}

/// SPL `TransferChecked`.
pub fn transfer_checked(
    token_program: &Pubkey,
    source: &Pubkey,
    mint: &Pubkey,
    dest: &Pubkey,
    authority: &Pubkey,
    amount: u64,
    decimals: u8,
) -> Instruction {
    let mut data = vec![12];
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);
    Instruction {
        program_id: *token_program,
        accounts: vec![
            AccountMeta::new(*source, false, true),
            AccountMeta::new(*mint, false, false),
            AccountMeta::new(*dest, false, true),
            AccountMeta::new(*authority, true, false),
        ],
        data,
    }
}

/// Account owner and lamports, or None when the account does not exist.
async fn account(nodes: &Nodes, key: &str) -> Result<Option<(String, u64)>> {
    let v = nodes.rpc("getAccountInfo", json!([key, { "encoding": "base64", "commitment": "confirmed" }])).await?;
    let value = v.get("value").cloned().unwrap_or(Value::Null);
    if value.is_null() {
        return Ok(None);
    }
    let owner = value.get("owner").and_then(Value::as_str).unwrap_or_default().to_string();
    Ok(Some((owner, as_u64(value.get("lamports")).unwrap_or(0))))
}

async fn rent_minimum(nodes: &Nodes, len: u64) -> Result<u64> {
    as_u64(Some(&nodes.rpc("getMinimumBalanceForRentExemption", json!([len])).await?))
        .ok_or_else(|| Error::Driver("getMinimumBalanceForRentExemption: bad answer".into()))
}

async fn balance(nodes: &Nodes, key: &str) -> Result<u64> {
    let v = nodes.rpc("getBalance", json!([key, { "commitment": "confirmed" }])).await?;
    as_u64(v.get("value")).ok_or_else(|| Error::Driver("getBalance: bad answer".into()))
}

async fn token_balance(nodes: &Nodes, token_account: &str) -> Result<u64> {
    match nodes.rpc("getTokenAccountBalance", json!([token_account, { "commitment": "confirmed" }])).await {
        Ok(v) => Ok(v.pointer("/value/amount").and_then(Value::as_str).and_then(|s| s.parse().ok()).unwrap_or(0)),
        // No token account: no tokens.
        Err(Error::Driver(msg)) if msg.contains("could not find account") || msg.contains("Invalid param") => Ok(0),
        Err(e) => Err(e),
    }
}

fn is_token_program(owner: &str) -> bool {
    owner == TOKEN_PROGRAM || owner == TOKEN_2022_PROGRAM
}

fn refuse_token_account(to: &str) -> Error {
    Error::BadRequest(format!("`{to}` is a token account. Send to the wallet address of the owner."))
}

#[async_trait]
impl PayoutChain for Sol {
    async fn sign(&self, ctx: &SignCtx<'_>, _conn: &mut PgConnection, _mode: SignMode<'_>) -> Result<NewAttempt> {
        let nodes = ctx.nodes;
        let seed: [u8; 32] = ctx.key.secret.as_slice().try_into().map_err(|_| Error::Internal("hot key is not 32 bytes".into()))?;
        let signer = SigningKey::from_bytes(&seed);
        let from = signer.verifying_key().to_bytes();
        let from_b58 = encode_pubkey(&from);
        if from_b58 != ctx.key.address {
            return Err(Error::Internal("the hot key does not match its address".into()));
        }
        let to_b58 = ctx.payout.address.trim().to_string();
        let to = decode_pubkey(&to_b58)?;
        let recipient = account(nodes, &to_b58).await?;
        if recipient.as_ref().is_some_and(|(owner, _)| is_token_program(owner)) {
            return Err(refuse_token_account(&to_b58));
        }
        let raw = to_base_units(ctx.payout.amount, ctx.an.decimals as u32)?;
        let amount = u64::try_from(raw).map_err(|_| Error::BadRequest("The amount is too large.".into()))?;
        let fee = 5_000u64;
        let hot_rent = rent_minimum(nodes, 0).await?;
        let hot_balance = balance(nodes, &from_b58).await?;

        let instructions = match &ctx.an.contract_address {
            None => {
                let new_account = recipient.as_ref().is_none_or(|(_, lamports)| *lamports == 0);
                if new_account && amount < hot_rent {
                    return Err(Error::BadRequest(format!(
                        "{to_b58} is a new account. A payout to it must be at least {} SOL, the rent-exempt minimum.",
                        from_base_units(hot_rent as u128, DECIMALS)
                    )));
                }
                let need = amount as u128 + fee as u128 + hot_rent as u128;
                if (hot_balance as u128) < need {
                    return Err(Error::BadRequest(format!(
                        "The hot wallet holds {} SOL. This payout needs {} SOL with the fee and the wallet's own rent reserve.",
                        from_base_units(hot_balance as u128, DECIMALS),
                        from_base_units(need, DECIMALS)
                    )));
                }
                vec![system_transfer(&from, &to, amount)]
            }
            Some(mint_b58) => {
                let mint = decode_pubkey(mint_b58)?;
                let (mint_owner, _) = account(nodes, mint_b58)
                    .await?
                    .ok_or_else(|| Error::Unavailable(format!("The token mint {mint_b58} does not exist on this network.")))?;
                if !is_token_program(&mint_owner) {
                    return Err(Error::Unavailable(format!("{mint_b58} is not a token mint.")));
                }
                let token_program = decode_pubkey(&mint_owner)?;
                let source = associated_token_address(&from, &mint, &token_program);
                let have = token_balance(nodes, &encode_pubkey(&source)).await?;
                if have < amount {
                    return Err(Error::BadRequest(format!(
                        "The hot wallet holds {} {}. This payout needs {}.",
                        from_base_units(have as u128, ctx.an.decimals as u32),
                        ctx.an.asset.symbol,
                        ctx.payout.amount.normalize()
                    )));
                }
                let dest = associated_token_address(&to, &mint, &token_program);
                let mut ixs = Vec::new();
                let mut rent = 0u64;
                if account(nodes, &encode_pubkey(&dest)).await?.is_none() {
                    let len = if mint_owner == TOKEN_2022_PROGRAM { TOKEN_2022_ACCOUNT_LEN } else { TOKEN_ACCOUNT_LEN };
                    rent = rent_minimum(nodes, len).await?;
                    ixs.push(create_ata_idempotent(&from, &dest, &to, &mint, &token_program));
                }
                let need = fee as u128 + rent as u128 + hot_rent as u128;
                if (hot_balance as u128) < need {
                    return Err(Error::BadRequest(format!(
                        "The hot wallet holds {} SOL. This payout needs {} SOL for the fee{}.",
                        from_base_units(hot_balance as u128, DECIMALS),
                        from_base_units(need, DECIMALS),
                        if rent > 0 { " and the recipient's token account" } else { "" }
                    )));
                }
                ixs.push(transfer_checked(&token_program, &source, &mint, &dest, &from, amount, ctx.an.decimals as u8));
                ixs
            }
        };

        let bh = nodes.rpc("getLatestBlockhash", json!([{ "commitment": "confirmed" }])).await?;
        let blockhash_b58 = bh.pointer("/value/blockhash").and_then(Value::as_str).ok_or_else(|| Error::Driver("no blockhash".into()))?;
        let last_valid =
            as_u64(bh.pointer("/value/lastValidBlockHeight")).ok_or_else(|| Error::Driver("no lastValidBlockHeight".into()))?;
        let blockhash: [u8; 32] = bs58::decode(blockhash_b58)
            .into_vec()
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| Error::Driver("bad blockhash".into()))?;
        let (message, signers) = compile_message(&from, &instructions, &blockhash);
        if signers != 1 {
            return Err(Error::Internal("a payout transaction has one signer".into()));
        }
        let (tx, sig) = sign_transaction(&message, &[&signer]);
        Ok(NewAttempt {
            from_address: from_b58,
            txid: sig,
            raw_tx: base64::engine::general_purpose::STANDARD.encode(tx),
            nonce: None,
            params: json!({ "blockhash": blockhash_b58, "last_valid_block_height": last_valid }),
            valid_until: None,
        })
    }

    async fn broadcast(&self, nodes: &Nodes, a: &AttemptRow) -> Broadcast {
        let mut refusals = Vec::new();
        let mut unreachable = Vec::new();
        let opts = json!({ "encoding": "base64", "preflightCommitment": "confirmed", "maxRetries": 10 });
        for url in &nodes.urls {
            match json_rpc(&nodes.http, url, "sendTransaction", json!([a.raw_tx, opts])).await {
                Ok(_) => return Broadcast::Accepted,
                Err(Error::Driver(msg)) if msg.contains("already been processed") => return Broadcast::Accepted,
                Err(Error::Driver(msg)) => refusals.push(msg),
                Err(e) => unreachable.push(e.to_string()),
            }
        }
        if refusals.is_empty() { Broadcast::Unreachable(unreachable.join("; ")) } else { Broadcast::Refused(refusals.join("; ")) }
    }

    async fn state(&self, nodes: &Nodes, a: &AttemptRow) -> Result<TxState> {
        let mut answered = false;
        let mut pending = false;
        for url in &nodes.urls {
            let Ok(v) = json_rpc(&nodes.http, url, "getSignatureStatuses", json!([[a.txid], { "searchTransactionHistory": true }])).await
            else {
                continue;
            };
            answered = true;
            let st = v.pointer("/value/0").cloned().unwrap_or(Value::Null);
            if st.is_null() {
                continue;
            }
            let level = st.get("confirmationStatus").and_then(Value::as_str).unwrap_or("processed");
            if level == "processed" {
                pending = true;
                continue;
            }
            let confirmations = match st.get("confirmations") {
                Some(Value::Number(n)) => n.as_i64().unwrap_or(0) + 1,
                // Finalized (rooted): as final as Solana gets.
                _ => 1_000,
            };
            let err = st.get("err").cloned().unwrap_or(Value::Null);
            let fee = json_rpc(
                &nodes.http,
                url,
                "getTransaction",
                json!([a.txid, { "encoding": "json", "commitment": "confirmed", "maxSupportedTransactionVersion": 0 }]),
            )
            .await
            .ok()
            .and_then(|t| as_u64(t.pointer("/meta/fee")))
            .map(|l| from_base_units(l as u128, DECIMALS));
            return Ok(TxState::Included {
                confirmations,
                success: err.is_null(),
                fee,
                error: (!err.is_null()).then(|| format!("The transaction failed on the chain: {err}")),
            });
        }
        if !answered {
            return Err(Error::Unavailable(format!("no {} node answered", nodes.network.name)));
        }
        Ok(if pending { TxState::Pending } else { TxState::Unknown })
    }

    async fn landing(&self, nodes: &Nodes, a: &AttemptRow) -> Result<Landing> {
        let Some(last_valid) = as_u64(a.params.get("last_valid_block_height")) else { return Ok(Landing::CanLand) };
        let height = as_u64(Some(&nodes.rpc("getBlockHeight", json!([{ "commitment": "finalized" }])).await?))
            .ok_or_else(|| Error::Driver("getBlockHeight: bad answer".into()))?;
        Ok(if height > last_valid { Landing::Expired } else { Landing::CanLand })
    }

    async fn estimate_fee(&self, nodes: &Nodes, _an: &AssetNetwork, from: Option<&str>) -> Result<Decimal> {
        // The fee of a one-signature message, from the chain.
        let payer = match from {
            Some(f) => decode_pubkey(f)?,
            None => [1u8; 32],
        };
        let bh = nodes.rpc("getLatestBlockhash", json!([{ "commitment": "confirmed" }])).await?;
        let blockhash: [u8; 32] = bh
            .pointer("/value/blockhash")
            .and_then(Value::as_str)
            .and_then(|s| bs58::decode(s).into_vec().ok())
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| Error::Driver("no blockhash".into()))?;
        let (message, _) = compile_message(&payer, &[system_transfer(&payer, &[2u8; 32], 1)], &blockhash);
        let v = nodes
            .rpc("getFeeForMessage", json!([base64::engine::general_purpose::STANDARD.encode(message), { "commitment": "confirmed" }]))
            .await?;
        let lamports = as_u64(v.get("value")).unwrap_or(5_000);
        Ok(from_base_units(lamports as u128, DECIMALS))
    }

    async fn native_balance(&self, nodes: &Nodes, address: &str) -> Result<Decimal> {
        Ok(from_base_units(balance(nodes, address).await? as u128, DECIMALS))
    }

    async fn precheck(&self, nodes: &Nodes, an: &AssetNetwork, to: &str, amount: Decimal) -> Result<()> {
        let recipient = account(nodes, to).await?;
        if recipient.as_ref().is_some_and(|(owner, _)| is_token_program(owner)) {
            return Err(refuse_token_account(to));
        }
        if an.is_native() && recipient.as_ref().is_none_or(|(_, lamports)| *lamports == 0) {
            let min = rent_minimum(nodes, 0).await?;
            if to_base_units(amount, DECIMALS)? < min as u128 {
                return Err(Error::BadRequest(format!(
                    "{to} is a new account. A payout to it must be at least {} SOL, the rent-exempt minimum.",
                    from_base_units(min as u128, DECIMALS)
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_program_addresses() {
        assert_eq!(encode_pubkey(&SYSTEM_PROGRAM), "11111111111111111111111111111111");
        assert_eq!(encode_pubkey(&program(TOKEN_PROGRAM)), TOKEN_PROGRAM);
    }

    #[test]
    fn program_addresses_are_off_curve() {
        let owner = decode_pubkey("9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM").unwrap();
        let mint = decode_pubkey("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap();
        let ata = associated_token_address(&owner, &mint, &program(TOKEN_PROGRAM));
        assert!(!on_curve(&ata));
        assert!(on_curve(&owner));
        assert_ne!(ata, associated_token_address(&owner, &mint, &program(TOKEN_2022_PROGRAM)));
    }

    #[test]
    fn message_layout() {
        let payer = [9u8; 32];
        let to = [8u8; 32];
        let (m, signers) = compile_message(&payer, &[system_transfer(&payer, &to, 5)], &[7u8; 32]);
        assert_eq!(signers, 1);
        // header: 1 signer, 0 read-only signed, 1 read-only unsigned (the system program).
        assert_eq!(&m[..4], &[1, 0, 1, 3]);
        assert_eq!(&m[4..36], &payer);
        assert_eq!(&m[36..68], &to);
        assert_eq!(&m[68..100], &SYSTEM_PROGRAM);
        assert_eq!(&m[100..132], &[7u8; 32]);
        // one instruction: program 2, accounts [0, 1], 12 bytes of data.
        assert_eq!(&m[132..138], &[1, 2, 2, 0, 1, 12]);
    }

    #[test]
    fn token_transfer_orders_accounts() {
        let payer = [9u8; 32];
        let owner = [5u8; 32];
        let mint = [4u8; 32];
        let tp = program(TOKEN_PROGRAM);
        let src = [3u8; 32];
        let dest = [2u8; 32];
        let ixs = [create_ata_idempotent(&payer, &dest, &owner, &mint, &tp), transfer_checked(&tp, &src, &mint, &dest, &payer, 10, 6)];
        let (m, signers) = compile_message(&payer, &ixs, &[0u8; 32]);
        assert_eq!(signers, 1);
        // Writable non-signers: dest, src. Read-only: owner, mint, system, token program, ATA program.
        assert_eq!(&m[..3], &[1, 0, 5]);
        assert_eq!(m[3], 8);
    }

    #[test]
    fn address_of_seed() {
        // RFC 8032 test 1.
        let seed = hex::decode("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60").unwrap();
        let addr = address_from_secret(&seed).unwrap();
        assert_eq!(hex::encode(decode_pubkey(&addr).unwrap()), "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
    }
}
