//! Solana driver over JSON-RPC: SOL and SPL tokens. Transactions are encoded and
//! signed here (ed25519-dalek), without the Solana SDK.
//!
//! Deposit owner per payment: an ed25519 key from the deposit seed and the index.
//! SPL tokens land in the owner's associated token account (ATA) for the mint.
//! Detection reads the signatures of the watched account (`getSignaturesForAddress`)
//! and the balance change in each transaction. The `confirmed` and `finalized`
//! commitments map to confirmations.
//!
//! SOL sweep: the deposit pays its own fee and keeps nothing. SPL sweep: the
//! project's hot wallet pays the fee and creates the settlement ATA when it is
//! missing; the deposit ATA is then closed and its rent goes to the hot wallet.

use std::{collections::HashMap, sync::Mutex, time::Duration};

use async_trait::async_trait;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::{Value, json};
use sha2::{Digest, Sha256, Sha512};
use zeroize::Zeroizing;

use crate::{
    chains::driver::{
        AssetContext, DerivedAddress, Observation, PaymentDriver, SweepCheck, SweepRequest, SweepResult, SweepState, WalletContext,
    },
    error::{Error, Result},
};

const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
const FEE_PER_SIGNATURE: u64 = 5_000;
/// Rent-exempt minimum of an account without data.
const RENT_EXEMPT_EMPTY: u64 = 890_880;
/// Confirmations reported for a finalized transaction.
const FINAL_CONFIRMATIONS: i32 = 32;
/// Size of an SPL mint account.
const MINT_SIZE: u64 = 82;

const SYSTEM_PROGRAM: &str = "11111111111111111111111111111111";
const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
const ATA_PROGRAM: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
const MAINNET_GENESIS: &str = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d";
const DEVNET_GENESIS: &str = "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG";

type Pubkey = [u8; 32];

pub struct SolDriver {
    client: reqwest::Client,
    rpc_url: String,
    master_seed: Option<Zeroizing<String>>,
    confirmations_required: i32,
    /// Token program that owns each mint.
    token_programs: Mutex<HashMap<Pubkey, Pubkey>>,
    /// Balance change of an account in a confirmed transaction: (signature, account).
    deltas: Mutex<HashMap<(String, Pubkey), i128>>,
}

/// An account an instruction uses.
#[derive(Clone, Copy)]
struct Meta {
    key: Pubkey,
    signer: bool,
    writable: bool,
}

struct Instruction {
    program: Pubkey,
    accounts: Vec<Meta>,
    data: Vec<u8>,
}

impl SolDriver {
    /// Refuses a node on another cluster than the mode's (devnet for testnet, mainnet-beta
    /// for mainnet), except a local validator in testnet mode. Without `master_seed`
    /// (the API process), the driver watches and validates but cannot derive or sweep.
    pub async fn connect(rpc_url: &str, master_seed: Option<&str>, testnet: bool, confirmations_required: i32) -> Result<Self> {
        let d = Self {
            client: reqwest::Client::builder().timeout(Duration::from_secs(20)).build().map_err(|e| Error::Internal(e.to_string()))?,
            rpc_url: rpc_url.trim().to_string(),
            master_seed: master_seed.map(|s| Zeroizing::new(s.to_string())),
            confirmations_required,
            token_programs: Mutex::new(HashMap::new()),
            deltas: Mutex::new(HashMap::new()),
        };
        let genesis = d.rpc("getGenesisHash", json!([])).await?;
        let genesis = genesis.as_str().unwrap_or_default();
        let (expected, cluster) = if testnet { (DEVNET_GENESIS, "devnet") } else { (MAINNET_GENESIS, "mainnet-beta") };
        if genesis != expected && !(testnet && is_local(&d.rpc_url)) {
            return Err(Error::Driver(format!("solana: this node is not on {cluster} (genesis hash {genesis})")));
        }
        Ok(d)
    }

    async fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let resp = self.client.post(&self.rpc_url).json(&body).send().await.map_err(|e| Error::Driver(format!("solana {method}: {e}")))?;
        let v: Value = resp.json().await.map_err(|e| Error::Driver(format!("solana {method}: {e}")))?;
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            return Err(Error::Driver(format!("solana {method}: {err}")));
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }

    fn seed(&self) -> Result<&str> {
        self.master_seed
            .as_deref()
            .map(|s| s.as_str())
            .ok_or_else(|| Error::Driver("solana: the deposit seed is only available in the worker".into()))
    }

    async fn balance(&self, key: &Pubkey) -> Result<u64> {
        let res = self.rpc("getBalance", json!([b58(key), { "commitment": "confirmed" }])).await?;
        Ok(res.get("value").and_then(Value::as_u64).unwrap_or(0))
    }

    /// Owner program of an account, or None when the account does not exist.
    async fn account_owner(&self, key: &Pubkey) -> Result<Option<String>> {
        let res = self.rpc("getAccountInfo", json!([b58(key), { "encoding": "base64", "commitment": "confirmed" }])).await?;
        Ok(res.pointer("/value/owner").and_then(Value::as_str).map(str::to_string))
    }

    async fn token_program(&self, mint: &Pubkey) -> Result<Pubkey> {
        if let Some(p) = self.token_programs.lock().unwrap_or_else(|e| e.into_inner()).get(mint) {
            return Ok(*p);
        }
        let owner = self.account_owner(mint).await?.ok_or_else(|| Error::Driver(format!("solana: mint {} not found", b58(mint))))?;
        if owner != TOKEN_PROGRAM && owner != TOKEN_2022_PROGRAM {
            return Err(Error::Driver(format!("solana: {} is not a token mint", b58(mint))));
        }
        let program = decode_pubkey(&owner)?;
        self.token_programs.lock().unwrap_or_else(|e| e.into_inner()).insert(*mint, program);
        Ok(program)
    }

    /// Token balance of a token account in base units (0 when it does not exist).
    async fn token_account_balance(&self, account: &Pubkey) -> Result<u64> {
        match self.rpc("getTokenAccountBalance", json!([b58(account), { "commitment": "confirmed" }])).await {
            Ok(v) => Ok(v.pointer("/value/amount").and_then(Value::as_str).and_then(|s| s.parse().ok()).unwrap_or(0)),
            Err(e) if e.to_string().contains("could not find account") || e.to_string().contains("Invalid param") => Ok(0),
            Err(e) => Err(e),
        }
    }

    async fn mint_decimals(&self, mint: &Pubkey) -> Result<u8> {
        let v = self.rpc("getTokenSupply", json!([b58(mint), { "commitment": "confirmed" }])).await?;
        v.pointer("/value/decimals")
            .and_then(Value::as_u64)
            .map(|d| d as u8)
            .ok_or_else(|| Error::Driver(format!("solana: no decimals for mint {}", b58(mint))))
    }

    async fn ensure_not_token_account(&self, key: &Pubkey) -> Result<()> {
        if let Some(owner) = self.account_owner(key).await?
            && (owner == TOKEN_PROGRAM || owner == TOKEN_2022_PROGRAM)
        {
            return Err(Error::Driver(format!("solana: {} is a token account; settle to the wallet address that owns it", b58(key))));
        }
        Ok(())
    }

    /// Base-unit change of `account` in a transaction: lamports, or tokens of `mint`.
    /// None while the node does not have the transaction yet.
    async fn delta(&self, signature: &str, account: &Pubkey, mint: Option<&Pubkey>) -> Result<Option<i128>> {
        let cache_key = (signature.to_string(), *account);
        if let Some(d) = self.deltas.lock().unwrap_or_else(|e| e.into_inner()).get(&cache_key) {
            return Ok(Some(*d));
        }
        let tx = self
            .rpc(
                "getTransaction",
                json!([signature, { "encoding": "json", "commitment": "confirmed", "maxSupportedTransactionVersion": 0 }]),
            )
            .await?;
        if tx.is_null() {
            return Ok(None);
        }
        let account_b58 = b58(account);
        let mut keys: Vec<&str> = Vec::new();
        for path in ["/transaction/message/accountKeys", "/meta/loadedAddresses/writable", "/meta/loadedAddresses/readonly"] {
            keys.extend(tx.pointer(path).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str));
        }
        let Some(index) = keys.iter().position(|k| *k == account_b58) else { return Ok(Some(0)) };
        let delta = match mint {
            None => {
                let at =
                    |path: &str| tx.pointer(path).and_then(Value::as_array).and_then(|a| a.get(index)).and_then(Value::as_u64).unwrap_or(0);
                at("/meta/postBalances") as i128 - at("/meta/preBalances") as i128
            }
            Some(mint) => {
                let mint_b58 = b58(mint);
                let amount = |path: &str| {
                    tx.pointer(path)
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .find(|b| {
                            b.get("accountIndex").and_then(Value::as_u64) == Some(index as u64)
                                && b.get("mint").and_then(Value::as_str) == Some(mint_b58.as_str())
                        })
                        .and_then(|b| b.pointer("/uiTokenAmount/amount"))
                        .and_then(Value::as_str)
                        .and_then(|s| s.parse::<i128>().ok())
                        .unwrap_or(0)
                };
                amount("/meta/postTokenBalances") - amount("/meta/preTokenBalances")
            }
        };
        let mut cache = self.deltas.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() > 50_000 {
            cache.clear();
        }
        cache.insert(cache_key, delta);
        Ok(Some(delta))
    }

    /// Sign with `signers` and send; wait until the transaction is confirmed, fails, or
    /// its blockhash expires. Returns the signature.
    async fn send(&self, payer: &Pubkey, signers: &[&SigningKey], instructions: &[Instruction]) -> Result<String> {
        let bh = self.rpc("getLatestBlockhash", json!([{ "commitment": "confirmed" }])).await?;
        let blockhash = bh
            .pointer("/value/blockhash")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Driver("solana: no blockhash".into()))
            .and_then(decode_pubkey)?;
        let last_valid = bh.pointer("/value/lastValidBlockHeight").and_then(Value::as_u64).unwrap_or(u64::MAX);
        let (message, signer_keys) = compile(payer, instructions, &blockhash);
        let mut tx = Vec::with_capacity(1 + 64 * signer_keys.len() + message.len());
        compact_u16(&mut tx, signer_keys.len());
        for key in &signer_keys {
            let signer = signers
                .iter()
                .find(|s| s.verifying_key().to_bytes() == *key)
                .ok_or_else(|| Error::Internal(format!("solana: no key to sign for {}", b58(key))))?;
            tx.extend_from_slice(&signer.sign(&message).to_bytes());
        }
        tx.extend_from_slice(&message);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&tx);
        let sig = self.rpc("sendTransaction", json!([encoded, { "encoding": "base64", "preflightCommitment": "confirmed" }])).await?;
        let sig = sig.as_str().unwrap_or_default().to_string();
        for _ in 0..90 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let Ok(height) = self.rpc("getBlockHeight", json!([{ "commitment": "confirmed" }])).await else { continue };
            let Ok(st) = self.rpc("getSignatureStatuses", json!([[sig]])).await else { continue };
            match st.pointer("/value/0").filter(|s| !s.is_null()) {
                Some(s) => {
                    if let Some(err) = s.get("err").filter(|e| !e.is_null()) {
                        return Err(Error::Driver(format!("solana transaction {sig} failed: {err}")));
                    }
                    if matches!(s.get("confirmationStatus").and_then(Value::as_str), Some("confirmed" | "finalized")) {
                        return Ok(sig);
                    }
                }
                None if height.as_u64().unwrap_or(0) > last_valid => {
                    return Err(Error::Driver(format!("solana transaction {sig} expired before it landed")));
                }
                None => {}
            }
        }
        tracing::warn!(%sig, "solana transaction not confirmed within 90 s");
        Ok(sig)
    }

    async fn sweep_sol(&self, signer: &SigningKey, to: &Pubkey, req: &SweepRequest) -> Result<SweepResult> {
        let from = signer.verifying_key().to_bytes();
        let balance = self.balance(&from).await?;
        if balance <= FEE_PER_SIGNATURE {
            return Err(Error::Driver("solana: nothing to sweep after fees".into()));
        }
        let wanted = sol_to_lamports(req.amount).min(balance);
        let mut send = wanted.saturating_sub(FEE_PER_SIGNATURE);
        // An account must hold 0 or at least the rent-exempt minimum: take dust along.
        let left = balance - send - FEE_PER_SIGNATURE;
        if left > 0 && left < RENT_EXEMPT_EMPTY {
            send = balance - FEE_PER_SIGNATURE;
        }
        if send == 0 {
            return Err(Error::Driver("solana: nothing to sweep after fees".into()));
        }
        let txid = self.send(&from, &[signer], &[system_transfer(&from, to, send)]).await?;
        Ok(SweepResult { txid, network_fee: lamports_to_sol(wanted.saturating_sub(send)) })
    }

    async fn sweep_spl(
        &self,
        w: &WalletContext,
        signer: &SigningKey,
        to_owner: &Pubkey,
        mint: &Pubkey,
        req: &SweepRequest,
        asset: &AssetContext,
    ) -> Result<SweepResult> {
        let hot = hot_key(w)?;
        let hot_pub = hot.verifying_key().to_bytes();
        let owner = signer.verifying_key().to_bytes();
        let program = self.token_program(mint).await?;
        let source = associated_token_address(&owner, mint, &program);
        let dest = associated_token_address(to_owner, mint, &program);
        let balance = self.token_account_balance(&source).await?;
        let amount = (units_from_decimal(req.amount, asset.decimals).min(balance as u128)) as u64;
        if amount == 0 {
            return Err(Error::Driver("solana: nothing to sweep".into()));
        }
        let mut ixs = vec![
            create_ata_idempotent(&hot_pub, &dest, to_owner, mint, &program),
            transfer_checked(&program, &source, mint, &dest, &owner, amount, asset.decimals as u8),
        ];
        // An empty deposit account is closed; its rent goes to the hot wallet.
        if amount == balance {
            ixs.push(close_account(&program, &source, &hot_pub, &owner));
        }
        let before = self.balance(&hot_pub).await.ok();
        let txid = self.send(&hot_pub, &[&hot, signer], &ixs).await?;
        if let (Some(before), Ok(after)) = (before, self.balance(&hot_pub).await) {
            tracing::info!(
                %txid,
                sol_cost = %(Decimal::from(before as i64 - after as i64) / Decimal::from(LAMPORTS_PER_SOL)),
                "solana token sweep: SOL from the hot wallet (negative: rent returned)"
            );
        }
        Ok(SweepResult { txid, network_fee: Decimal::ZERO })
    }

    // ---- tools (tests, funding) -------------------------------------------

    /// Send lamports from a key (32-byte ed25519 seed).
    pub async fn send_from_secret(&self, secret: &[u8], to: &str, lamports: u64) -> Result<String> {
        let signer = key_from_secret(secret)?;
        let from = signer.verifying_key().to_bytes();
        self.send(&from, &[&signer], &[system_transfer(&from, &decode_pubkey(to)?, lamports)]).await
    }

    /// Send tokens from a key's ATA to `to_owner`'s ATA, creating it as wallets do.
    pub async fn send_token_from_secret(&self, secret: &[u8], mint: &str, to_owner: &str, amount: u64) -> Result<String> {
        let signer = key_from_secret(secret)?;
        let from = signer.verifying_key().to_bytes();
        let (mint, to_owner) = (decode_pubkey(mint)?, decode_pubkey(to_owner)?);
        let program = self.token_program(&mint).await?;
        let decimals = self.mint_decimals(&mint).await?;
        let source = associated_token_address(&from, &mint, &program);
        let dest = associated_token_address(&to_owner, &mint, &program);
        let ixs = [
            create_ata_idempotent(&from, &dest, &to_owner, &mint, &program),
            transfer_checked(&program, &source, &mint, &dest, &from, amount, decimals),
        ];
        self.send(&from, &[&signer], &ixs).await
    }

    /// Create a mint (classic token program) whose authority is `payer`.
    pub async fn create_mint(&self, payer_secret: &[u8], mint_secret: &[u8], decimals: u8) -> Result<String> {
        let payer = key_from_secret(payer_secret)?;
        let mint = key_from_secret(mint_secret)?;
        let (payer_pub, mint_pub) = (payer.verifying_key().to_bytes(), mint.verifying_key().to_bytes());
        let program = decode_pubkey(TOKEN_PROGRAM)?;
        let rent = self.rpc("getMinimumBalanceForRentExemption", json!([MINT_SIZE])).await?.as_u64().unwrap_or(1_461_600);
        let mut create = vec![0, 0, 0, 0];
        create.extend_from_slice(&rent.to_le_bytes());
        create.extend_from_slice(&MINT_SIZE.to_le_bytes());
        create.extend_from_slice(&program);
        let mut init = vec![20, decimals];
        init.extend_from_slice(&payer_pub);
        init.push(0);
        let ixs = [
            Instruction { program: decode_pubkey(SYSTEM_PROGRAM)?, accounts: vec![signer_w(payer_pub), signer_w(mint_pub)], data: create },
            Instruction { program, accounts: vec![writable(mint_pub)], data: init },
        ];
        self.send(&payer_pub, &[&payer, &mint], &ixs).await?;
        Ok(b58(&mint_pub))
    }

    /// Mint tokens to `owner`'s ATA (created when missing). `authority` pays.
    pub async fn mint_to(&self, authority_secret: &[u8], mint: &str, owner: &str, amount: u64) -> Result<String> {
        let authority = key_from_secret(authority_secret)?;
        let auth_pub = authority.verifying_key().to_bytes();
        let (mint, owner) = (decode_pubkey(mint)?, decode_pubkey(owner)?);
        let program = self.token_program(&mint).await?;
        let dest = associated_token_address(&owner, &mint, &program);
        let mut data = vec![7];
        data.extend_from_slice(&amount.to_le_bytes());
        let ixs = [
            create_ata_idempotent(&auth_pub, &dest, &owner, &mint, &program),
            Instruction { program, accounts: vec![writable(mint), writable(dest), signer_r(auth_pub)], data },
        ];
        self.send(&auth_pub, &[&authority], &ixs).await
    }

    /// Balance of an address in lamports.
    pub async fn balance_lamports(&self, address: &str) -> Result<u64> {
        self.balance(&decode_pubkey(address)?).await
    }

    /// Token balance of `owner`'s ATA for `mint`, in base units.
    pub async fn token_balance(&self, owner: &str, mint: &str) -> Result<u64> {
        let (owner, mint) = (decode_pubkey(owner)?, decode_pubkey(mint)?);
        let program = self.token_program(&mint).await?;
        self.token_account_balance(&associated_token_address(&owner, &mint, &program)).await
    }

    /// The ATA of `owner` for `mint`.
    pub async fn token_account(&self, owner: &str, mint: &str) -> Result<String> {
        let (owner, mint) = (decode_pubkey(owner)?, decode_pubkey(mint)?);
        let program = self.token_program(&mint).await?;
        Ok(b58(&associated_token_address(&owner, &mint, &program)))
    }
}

/// Solana address of a 32-byte ed25519 seed.
pub fn address_from_secret(secret: &[u8]) -> Result<String> {
    Ok(b58(&key_from_secret(secret)?.verifying_key().to_bytes()))
}

fn key_from_secret(secret: &[u8]) -> Result<SigningKey> {
    let seed: [u8; 32] =
        secret.get(..32).and_then(|b| b.try_into().ok()).ok_or_else(|| Error::Driver("solana key must be 32 bytes".into()))?;
    Ok(SigningKey::from_bytes(&seed))
}

fn hot_key(w: &WalletContext) -> Result<SigningKey> {
    let secret = w.hot_secret.as_ref().ok_or_else(|| {
        Error::Driver("solana: a token sweep needs a Solana hot wallet to pay the fees. Assign one to the project.".into())
    })?;
    key_from_secret(secret)
}

fn signing_key(seed: &str, index: u64) -> SigningKey {
    let mut h = Sha512::new();
    h.update(seed.as_bytes());
    h.update(b"/");
    h.update(index.to_le_bytes());
    let digest = h.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&digest[..32]);
    SigningKey::from_bytes(&key)
}

fn is_local(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .is_some_and(|h| h == "localhost" || h.trim_matches(['[', ']']).parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()))
}

fn b58(key: &Pubkey) -> String {
    bs58::encode(key).into_string()
}

fn decode_pubkey(addr: &str) -> Result<Pubkey> {
    bs58::decode(addr.trim())
        .into_vec()
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| Error::Driver(format!("bad solana address {addr}")))
}

fn program_id(addr: &str) -> Pubkey {
    decode_pubkey(addr).expect("valid program id")
}

/// Program-derived address (the first bump whose hash is off the curve).
fn find_program_address(seeds: &[&[u8]], program: &Pubkey) -> Pubkey {
    for bump in (0..=255u8).rev() {
        let mut h = Sha256::new();
        for s in seeds {
            h.update(s);
        }
        h.update([bump]);
        h.update(program);
        h.update(b"ProgramDerivedAddress");
        let mut key = [0u8; 32];
        key.copy_from_slice(&h.finalize());
        if VerifyingKey::from_bytes(&key).is_err() {
            return key;
        }
    }
    unreachable!("no program address for these seeds")
}

fn associated_token_address(owner: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    find_program_address(&[owner, token_program, mint], &program_id(ATA_PROGRAM))
}

fn signer_w(key: Pubkey) -> Meta {
    Meta { key, signer: true, writable: true }
}
fn signer_r(key: Pubkey) -> Meta {
    Meta { key, signer: true, writable: false }
}
fn writable(key: Pubkey) -> Meta {
    Meta { key, signer: false, writable: true }
}
fn readonly(key: Pubkey) -> Meta {
    Meta { key, signer: false, writable: false }
}

fn system_transfer(from: &Pubkey, to: &Pubkey, lamports: u64) -> Instruction {
    let mut data = vec![2, 0, 0, 0];
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction { program: program_id(SYSTEM_PROGRAM), accounts: vec![signer_w(*from), writable(*to)], data }
}

/// `CreateIdempotent` of the associated token account program.
fn create_ata_idempotent(payer: &Pubkey, ata: &Pubkey, owner: &Pubkey, mint: &Pubkey, program: &Pubkey) -> Instruction {
    Instruction {
        program: program_id(ATA_PROGRAM),
        accounts: vec![
            signer_w(*payer),
            writable(*ata),
            readonly(*owner),
            readonly(*mint),
            readonly(program_id(SYSTEM_PROGRAM)),
            readonly(*program),
        ],
        data: vec![1],
    }
}

fn transfer_checked(
    program: &Pubkey,
    source: &Pubkey,
    mint: &Pubkey,
    dest: &Pubkey,
    owner: &Pubkey,
    amount: u64,
    decimals: u8,
) -> Instruction {
    let mut data = vec![12];
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);
    Instruction { program: *program, accounts: vec![writable(*source), readonly(*mint), writable(*dest), signer_r(*owner)], data }
}

fn close_account(program: &Pubkey, account: &Pubkey, dest: &Pubkey, owner: &Pubkey) -> Instruction {
    Instruction { program: *program, accounts: vec![writable(*account), writable(*dest), signer_r(*owner)], data: vec![9] }
}

fn compact_u16(out: &mut Vec<u8>, mut n: usize) {
    loop {
        let mut byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return;
        }
        byte |= 0x80;
        out.push(byte);
    }
}

/// Legacy message: account keys ordered signer-writable, signer-readonly, writable,
/// readonly, the payer first. Returns the message and the signer keys in order.
fn compile(payer: &Pubkey, instructions: &[Instruction], blockhash: &Pubkey) -> (Vec<u8>, Vec<Pubkey>) {
    let mut keys: Vec<Meta> = vec![signer_w(*payer)];
    let mut add = |m: Meta| match keys.iter_mut().find(|k| k.key == m.key) {
        Some(k) => {
            k.signer |= m.signer;
            k.writable |= m.writable;
        }
        None => keys.push(m),
    };
    for ix in instructions {
        for m in &ix.accounts {
            add(*m);
        }
        add(readonly(ix.program));
    }
    keys.sort_by_key(|k| match (k.signer, k.writable) {
        (true, true) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (false, false) => 3,
    });
    let signers: Vec<Pubkey> = keys.iter().filter(|k| k.signer).map(|k| k.key).collect();
    let ro_signed = keys.iter().filter(|k| k.signer && !k.writable).count();
    let ro_unsigned = keys.iter().filter(|k| !k.signer && !k.writable).count();
    let index = |key: &Pubkey| keys.iter().position(|k| k.key == *key).expect("key listed") as u8;
    let mut m = vec![signers.len() as u8, ro_signed as u8, ro_unsigned as u8];
    compact_u16(&mut m, keys.len());
    for k in &keys {
        m.extend_from_slice(&k.key);
    }
    m.extend_from_slice(blockhash);
    compact_u16(&mut m, instructions.len());
    for ix in instructions {
        m.push(index(&ix.program));
        compact_u16(&mut m, ix.accounts.len());
        for a in &ix.accounts {
            m.push(index(&a.key));
        }
        compact_u16(&mut m, ix.data.len());
        m.extend_from_slice(&ix.data);
    }
    (m, signers)
}

fn lamports_to_sol(l: u64) -> Decimal {
    Decimal::from(l) / Decimal::from(LAMPORTS_PER_SOL)
}

fn sol_to_lamports(d: Decimal) -> u64 {
    (d * Decimal::from(LAMPORTS_PER_SOL)).floor().to_u64().unwrap_or(0)
}

fn units_from_decimal(d: Decimal, decimals: u32) -> u128 {
    (d * Decimal::from(10u64.pow(decimals))).floor().to_u128().unwrap_or(0)
}

#[async_trait]
impl PaymentDriver for SolDriver {
    fn chain(&self) -> &str {
        "solana"
    }

    fn can_derive(&self) -> bool {
        self.master_seed.is_some()
    }

    /// A system account address, or one not yet used. Token accounts and mints are refused:
    /// tokens must go to the wallet that owns the token account.
    async fn validate_address(&self, address: &str) -> Result<bool> {
        let Ok(key) = decode_pubkey(address) else { return Ok(false) };
        let owner = self.account_owner(&key).await?;
        Ok(!matches!(owner.as_deref(), Some(TOKEN_PROGRAM | TOKEN_2022_PROGRAM)))
    }

    async fn derive_deposit(&self, _w: &WalletContext, derivation_index: u64) -> Result<DerivedAddress> {
        Ok(DerivedAddress {
            address: b58(&signing_key(self.seed()?, derivation_index).verifying_key().to_bytes()),
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
        let owner = decode_pubkey(address)?;
        let (watched, mint) = match asset.contract_address.as_deref() {
            None => (owner, None),
            Some(m) => {
                let mint = decode_pubkey(m)?;
                (associated_token_address(&owner, &mint, &self.token_program(&mint).await?), Some(mint))
            }
        };
        let sigs = self.rpc("getSignaturesForAddress", json!([b58(&watched), { "limit": 50, "commitment": "confirmed" }])).await?;
        let sigs = sigs.as_array().cloned().unwrap_or_default();
        if sigs.is_empty() {
            return Ok(vec![]);
        }
        let tip = self.rpc("getSlot", json!([{ "commitment": "confirmed" }])).await?.as_u64().unwrap_or(0);
        let mut out = Vec::new();
        for s in sigs {
            let Some(signature) = s.get("signature").and_then(Value::as_str) else { continue };
            if s.get("err").is_some_and(|e| !e.is_null()) {
                continue;
            }
            let Some(delta) = self.delta(signature, &watched, mint.as_ref()).await?.filter(|d| *d > 0) else { continue };
            let slot = s.get("slot").and_then(Value::as_u64).unwrap_or(tip);
            let confirmations = match s.get("confirmationStatus").and_then(Value::as_str) {
                Some("finalized") => FINAL_CONFIRMATIONS.max(self.confirmations_required),
                Some("confirmed") => (tip.saturating_sub(slot) + 1).min(FINAL_CONFIRMATIONS as u64 - 1) as i32,
                _ => 0,
            };
            out.push(Observation {
                chain_tx_key: signature.to_string(),
                txid: signature.to_string(),
                vout: None,
                amount: Decimal::from_i128_with_scale(delta, asset.decimals).normalize(),
                confirmations,
                block_height: Some(slot as i64),
            });
        }
        Ok(out)
    }

    async fn sweep(&self, w: &WalletContext, req: &SweepRequest, asset: &AssetContext) -> Result<SweepResult> {
        let signer = signing_key(self.seed()?, req.derivation_index);
        if b58(&signer.verifying_key().to_bytes()) != req.from_address {
            return Err(Error::Driver(format!("solana: index {} does not derive {}", req.derivation_index, req.from_address)));
        }
        let to = decode_pubkey(&req.to_address)?;
        self.ensure_not_token_account(&to).await?;
        match asset.contract_address.as_deref() {
            None => self.sweep_sol(&signer, &to, req).await,
            Some(mint) => self.sweep_spl(w, &signer, &to, &decode_pubkey(mint)?, req, asset).await,
        }
    }

    /// Final once finalized. An error fails it; so does no status after its blockhash expired.
    async fn sweep_state(&self, sweep: &SweepCheck, _asset: &AssetContext) -> Result<SweepState> {
        let st = self.rpc("getSignatureStatuses", json!([[sweep.txid], { "searchTransactionHistory": true }])).await?;
        Ok(match st.pointer("/value/0").filter(|s| !s.is_null()) {
            Some(s) => match s.get("err").filter(|e| !e.is_null()) {
                Some(err) => SweepState::Failed(format!("the transaction failed on chain: {err}")),
                None if s.get("confirmationStatus").and_then(Value::as_str) == Some("finalized") => SweepState::Confirmed,
                None => SweepState::Pending,
            },
            None if sweep.expired() => SweepState::Failed("the transaction is not on chain and its blockhash has expired".into()),
            None => SweepState::Pending,
        })
    }

    async fn tip_height(&self) -> Result<i64> {
        Ok(self.rpc("getSlot", json!([{ "commitment": "confirmed" }])).await?.as_u64().unwrap_or(0) as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn driver(seed: Option<&str>) -> SolDriver {
        SolDriver {
            client: reqwest::Client::new(),
            rpc_url: String::new(),
            master_seed: seed.map(|s| Zeroizing::new(s.to_string())),
            confirmations_required: 1,
            token_programs: Mutex::new(HashMap::new()),
            deltas: Mutex::new(HashMap::new()),
        }
    }

    #[tokio::test]
    async fn derivation_is_stable_and_needs_the_seed() {
        let d = driver(Some("unit-test-seed"));
        let w = WalletContext::default();
        let a1 = d.derive_deposit(&w, 1).await.unwrap().address;
        assert_eq!(a1, b58(&signing_key("unit-test-seed", 1).verifying_key().to_bytes()));
        assert_ne!(a1, d.derive_deposit(&w, 2).await.unwrap().address);
        let api = driver(None);
        assert!(!api.can_derive());
        assert!(api.derive_deposit(&w, 1).await.is_err());
    }

    #[test]
    fn associated_token_address_matches_the_program() {
        // Created on a test validator by `CreateIdempotent`, which checks the derivation.
        let owner = decode_pubkey("9kHce5SuxqHaVsthkktSkjbHqC5TDoqWUXsk2H1SRWMN").unwrap();
        let mint = decode_pubkey("9Pb6xsZSFiughsY911ji3o2jai27dg6ZLL9a1KVLmqpQ").unwrap();
        let ata = associated_token_address(&owner, &mint, &program_id(TOKEN_PROGRAM));
        assert_eq!(b58(&ata), "42QGMt6FhGDkPYpe75WLeBomr24U6qqPLEHknBGkdctD");
    }

    #[test]
    fn associated_token_address_is_off_the_curve() {
        let owner = decode_pubkey("9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM").unwrap();
        let mint = decode_pubkey("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap();
        let ata = associated_token_address(&owner, &mint, &program_id(TOKEN_PROGRAM));
        assert!(VerifyingKey::from_bytes(&ata).is_err(), "off the curve");
        assert_eq!(ata, associated_token_address(&owner, &mint, &program_id(TOKEN_PROGRAM)));
    }

    #[test]
    fn compiles_a_transfer_message() {
        let from = [1u8; 32];
        let to = [2u8; 32];
        let (m, signers) = compile(&from, &[system_transfer(&from, &to, 7)], &[9u8; 32]);
        assert_eq!(signers, vec![from]);
        // header, 3 keys, blockhash, 1 instruction: program index 2, accounts [0, 1].
        assert_eq!(&m[..4], &[1, 0, 1, 3]);
        assert_eq!(&m[4 + 96 + 32..4 + 96 + 32 + 6], &[1, 2, 2, 0, 1, 12]);
    }

    #[test]
    fn compiles_a_token_sweep_with_two_signers() {
        let (hot, owner, dest_owner, mint) = ([1u8; 32], [2u8; 32], [3u8; 32], [4u8; 32]);
        let program = program_id(TOKEN_PROGRAM);
        let src = associated_token_address(&owner, &mint, &program);
        let dst = associated_token_address(&dest_owner, &mint, &program);
        let ixs = [
            create_ata_idempotent(&hot, &dst, &dest_owner, &mint, &program),
            transfer_checked(&program, &src, &mint, &dst, &owner, 5, 6),
            close_account(&program, &src, &hot, &owner),
        ];
        let (m, signers) = compile(&hot, &ixs, &[0u8; 32]);
        assert_eq!(signers, vec![hot, owner]);
        // 2 signers (owner read-only), read-only unsigned: dest owner, mint, system, token, ATA programs.
        assert_eq!(&m[..3], &[2, 1, 5]);
    }

    #[test]
    fn local_urls() {
        assert!(is_local("http://127.0.0.1:8899"));
        assert!(is_local("http://localhost:8899"));
        assert!(is_local("http://[::1]:8899"));
        assert!(!is_local("https://api.devnet.solana.com"));
    }
}
