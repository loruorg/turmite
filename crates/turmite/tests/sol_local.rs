#![cfg(feature = "sol")]
//! Solana on the local solana-test-validator (127.0.0.1:8899), in split roles: the API
//! takes deposit addresses from the pool that the worker fills. A customer pays SOL
//! and an SPL token; the worker confirms, finishes and sweeps to the settlement wallet.
//! Also: worker restart and counter continuity of the pool.

mod common;
mod roles;

use std::time::Duration;

use roles::{Split, random_secret};
use serde_json::{Value, json};
use turmite::chains::{
    driver::WalletContext,
    sol::{SolDriver, address_from_secret},
};

const RPC: &str = "http://127.0.0.1:8899";

async fn reachable() -> bool {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "getHealth" });
    match reqwest::Client::new().post(RPC).json(&body).send().await {
        Ok(r) => r.json::<Value>().await.is_ok_and(|v| v["result"] == "ok"),
        Err(_) => false,
    }
}

async fn airdrop(chain: &SolDriver, to: &str, lamports: u64) {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "requestAirdrop", "params": [to, lamports] });
    let r: Value = reqwest::Client::new().post(RPC).json(&body).send().await.unwrap().json().await.unwrap();
    assert!(r["result"].is_string(), "airdrop: {r}");
    for _ in 0..60 {
        if chain.balance_lamports(to).await.unwrap_or(0) >= lamports {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("airdrop to {to} did not arrive");
}

#[tokio::test]
async fn sol_split_roles_pool_sol_and_spl_sweeps() {
    if !reachable().await {
        eprintln!("SKIP: solana-test-validator not reachable at {RPC}");
        return;
    }
    let mut s = Split::start("SOL", RPC).await;
    let chain = SolDriver::connect(RPC, None, true, 1).await.unwrap();

    let settle_secret = random_secret();
    let settle = address_from_secret(&settle_secret).unwrap();
    let hot_secret = random_secret();
    let hot = address_from_secret(&hot_secret).unwrap();
    let payer_secret = random_secret();
    let payer = address_from_secret(&payer_secret).unwrap();
    airdrop(&chain, &hot, 1_000_000_000).await;
    airdrop(&chain, &payer, 2_000_000_000).await;
    let (_pid, key) = s.project("SOL", &settle, None, Some(("solana", &hot, &hot_secret))).await;
    let net = s.network("SOL").await;

    // The API process has no seed and the pool is empty: 503 with a retry hint.
    let order = json!({ "price_amount": 2, "price_currency": "usd", "pay_currency": "sol" });
    let (status, body, retry) = s.post("/v1/payment", &key, &order).await;
    assert_eq!(status, 503, "{body}");
    assert!(body["message"].as_str().unwrap_or_default().contains("Try again"), "{body}");
    assert_eq!(retry.as_deref(), Some("30"));

    // The worker fills the pool; the API takes an address from it.
    turmite::worker::pool::refill(&s.worker).await;
    assert_eq!(s.worker.store.pool_size(net.network_id).await.unwrap(), 25);
    let (status, p, _) = s.post("/v1/payment", &key, &order).await;
    assert_eq!(status, 201, "{p}");
    let id = common::pid(&p);
    let deposit = p["pay_address"].as_str().unwrap().to_string();
    let index = s.deposit_index(id).await;
    let worker_driver = s.worker.drivers().get("solana").unwrap();
    assert_eq!(worker_driver.derive_deposit(&WalletContext::default(), index).await.unwrap().address, deposit);
    assert_eq!(s.worker.store.pool_size(net.network_id).await.unwrap(), 24);

    // Native SOL: pay, confirm, finish, sweep. The deposit keeps nothing.
    let lamports = (p["pay_amount"].as_f64().unwrap() * 1e9).round() as u64;
    assert_eq!(lamports, 10_000_000);
    chain.send_from_secret(&payer_secret, &deposit, lamports).await.unwrap();
    let (p, sweeps) = s.settle(&key, id, 90).await;
    assert_eq!(p["payment_status"], "finished", "{p}");
    assert_eq!(sweeps.len(), 1);
    assert_eq!(chain.balance_lamports(&deposit).await.unwrap(), 0, "deposit emptied");
    assert_eq!(chain.balance_lamports(&settle).await.unwrap(), lamports - 5_000, "settlement received the SOL minus the fee");
    assert_eq!(p["outcome_amount"].as_f64().unwrap(), 0.01);
    assert_eq!(sweeps[0].amount, "0.009995".parse().unwrap());
    assert_eq!(sweeps[0].network_fee_amount, "0.000005".parse().unwrap());
    println!("SOL sweep {}: fee {} SOL paid by the deposit", sweeps[0].txid.as_deref().unwrap_or(""), sweeps[0].network_fee_amount);
    assert_eq!(s.sweeps_final(id, 60).await[0].status, "confirmed", "the sweep is finalized on chain");

    // SPL token: a test mint takes the place of USDC.
    let mint = chain.create_mint(&payer_secret, &random_secret(), 6).await.unwrap();
    chain.mint_to(&payer_secret, &mint, &payer, 100_000_000).await.unwrap();
    s.set_token_contract("usdcsol", &mint).await;
    let (status, p, _) =
        s.post("/v1/payment", &key, &json!({ "price_amount": 5, "price_currency": "usd", "pay_currency": "usdcsol" })).await;
    assert_eq!(status, 201, "{p}");
    let id = common::pid(&p);
    let deposit = p["pay_address"].as_str().unwrap().to_string();
    assert_eq!(p["pay_amount"].as_f64().unwrap(), 5.0);
    let deposit_ata = chain.token_account(&deposit, &mint).await.unwrap();
    chain.send_token_from_secret(&payer_secret, &mint, &deposit, 5_000_000).await.unwrap();
    let hot_before = chain.balance_lamports(&hot).await.unwrap();
    let (p, sweeps) = s.settle(&key, id, 90).await;
    assert_eq!(p["payment_status"], "finished", "{p}");
    assert_eq!(p["outcome_amount"].as_f64().unwrap(), 5.0);
    assert_eq!(sweeps.len(), 1);
    assert_eq!(chain.token_balance(&settle, &mint).await.unwrap(), 5_000_000, "settlement received the tokens");
    assert_eq!(chain.token_balance(&deposit, &mint).await.unwrap(), 0);
    assert_eq!(chain.balance_lamports(&deposit_ata).await.unwrap(), 0, "deposit token account closed");
    let hot_after = chain.balance_lamports(&hot).await.unwrap();
    println!(
        "SPL sweep {}: hot wallet {} -> {} lamports ({} net: fee 10000, settlement ATA rent paid, deposit ATA rent returned)",
        sweeps[0].txid.as_deref().unwrap_or(""),
        hot_before,
        hot_after,
        hot_before as i64 - hot_after as i64
    );
    println!("ATA vector: owner {settle} mint {mint} ata {}", chain.token_account(&settle, &mint).await.unwrap());
    // Nothing is left behind for the deposit owner either.
    assert_eq!(chain.balance_lamports(&deposit).await.unwrap(), 0);
    assert_eq!(s.sweeps_final(id, 60).await[0].status, "confirmed");

    // A token account is not a settlement address.
    let api_driver = s.api.drivers().get("solana").unwrap();
    let settle_ata = chain.token_account(&settle, &mint).await.unwrap();
    assert!(!api_driver.validate_address(&settle_ata).await.unwrap(), "token account refused");
    assert!(!api_driver.validate_address(&mint).await.unwrap(), "mint refused");
    assert!(api_driver.validate_address(&settle).await.unwrap());

    // Worker restart: the counter continues, also when its row is lost.
    let max_before: i64 = sqlx::query_scalar("SELECT MAX(derivation_index) FROM deposit_addresses WHERE network_id = $1")
        .bind(net.network_id)
        .fetch_one(&s.worker.store.pool)
        .await
        .unwrap();
    s.restart_worker().await;
    sqlx::query("DELETE FROM network_address_counter WHERE network_id = $1")
        .bind(net.network_id)
        .execute(&s.worker.store.pool)
        .await
        .unwrap();
    for _ in 0..3 {
        let (status, p, _) = s.post("/v1/payment", &key, &order).await;
        assert_eq!(status, 201, "{p}");
    }
    let free = s.worker.store.pool_size(net.network_id).await.unwrap();
    turmite::worker::pool::refill(&s.worker).await;
    assert_eq!(s.worker.store.pool_size(net.network_id).await.unwrap(), 25);
    let (count, distinct, max_after): (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COUNT(DISTINCT derivation_index), MAX(derivation_index) FROM deposit_addresses WHERE network_id = $1",
    )
    .bind(net.network_id)
    .fetch_one(&s.worker.store.pool)
    .await
    .unwrap();
    assert_eq!(count, distinct, "no index used twice");
    assert_eq!(max_after, max_before + (25 - free), "the counter continued after the restart");

    // A pool row that the seed does not derive is removed before the API can hand it out.
    s.worker.store.insert_deposit_address(net.network_id, None, 9_999, "11111111111111111111111111111112", None).await.unwrap();
    turmite::worker::pool::refill(&s.worker).await;
    let stray: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM deposit_addresses WHERE derivation_index = 9999")
        .fetch_one(&s.worker.store.pool)
        .await
        .unwrap();
    assert_eq!(stray, 0);
}
