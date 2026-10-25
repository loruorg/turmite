#![cfg(feature = "ton")]
//! TON on the local MyLocalTon chain (API v2 at 127.0.0.1:8082), in split roles. The
//! faucet pays a per-payment wallet address; the worker confirms, finishes and sweeps
//! by deploying the deposit wallet in the same message, with the settlement memo as a
//! text comment. A top-up after the sweep is swept again from the deployed wallet.

mod common;
mod roles;

use std::time::Duration;

use roles::Split;
use serde_json::{Value, json};
use tonlib_core::wallet::wallet_version::WalletVersion;
use turmite::chains::{
    AssetContext, PaymentDriver,
    driver::{SweepCheck, SweepState, WalletContext},
    ton::TonDriver,
};

const TON_API: &str = "http://127.0.0.1:8082";
// MyLocalTon masterchain faucet: wallet v3r2, id 42 (public test key).
const FAUCET_SECRET: &str = "a51e8fb6f0fae3834bf430f5012589d319e7b3b3303ceb82c816b762fccf2d05";
const MEMO: &str = "exchange-memo-4242";

/// Returns the hash of the external message.
async fn fund(faucet: &TonDriver, to: &str, nanoton: u64) -> String {
    // The faucet's seqno moves only once its last message lands: retry.
    for _ in 0..10 {
        if let Ok(hash) = faucet.send_from_secret(FAUCET_SECRET, WalletVersion::V3R2, 42, -1, to, nanoton).await {
            return hash;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    panic!("faucet could not send");
}

async fn wait_balance(driver: &TonDriver, address: &str, at_least: u64) -> u64 {
    let mut balance = 0;
    for _ in 0..40 {
        balance = driver.balance_nanoton(address).await.unwrap_or(0);
        if balance >= at_least {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    balance
}

/// Text comments of the incoming messages of an address.
async fn comments(address: &str) -> Vec<String> {
    let v: Value = reqwest::Client::new()
        .get(format!("{TON_API}/getTransactions"))
        .query(&[("address", address), ("limit", "10")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    v["result"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t.pointer("/in_msg/message").and_then(Value::as_str).map(str::to_string))
        .collect()
}

#[tokio::test]
async fn ton_split_roles_deploy_sweep_memo_and_topup() {
    if reqwest::get(format!("{TON_API}/getMasterchainInfo")).await.is_err() {
        eprintln!("SKIP: local TON API not reachable at {TON_API}");
        return;
    }
    let run = uuid::Uuid::new_v4().simple().to_string();
    let faucet = TonDriver::connect(TON_API, None, true, 1).await.unwrap();
    assert!(faucet.tip_height().await.unwrap() > 0);
    let err = TonDriver::connect(TON_API, None, false, 1).await.err().expect("a local chain is refused in mainnet mode");
    assert!(err.to_string().contains("not on mainnet"), "{err}");
    // Settlement: a fresh, undeployed wallet (as an exchange deposit address with a memo).
    let other = TonDriver::connect(TON_API, Some(&format!("payout-{run}")), true, 1).await.unwrap();
    let merchant = other.derive_deposit(&WalletContext::default(), 1).await.unwrap().address;

    let s = Split::start("TON", TON_API).await;
    let (_pid, key) = s.project("TON", &merchant, Some(MEMO), None).await;
    turmite::worker::pool::refill(&s.worker).await;

    let (status, p, _) = s.post("/v1/payment", &key, &json!({ "price_amount": 3, "price_currency": "usd", "pay_currency": "ton" })).await;
    assert_eq!(status, 201, "{p}");
    let id = common::pid(&p);
    let deposit = p["pay_address"].as_str().unwrap().to_string();
    assert!(deposit.starts_with("0Q") && deposit.len() == 48, "testnet non-bounceable address, got {deposit}");
    let worker_driver = s.worker.drivers().get("ton").unwrap();
    assert_eq!(worker_driver.derive_deposit(&WalletContext::default(), s.deposit_index(id).await).await.unwrap().address, deposit);
    let pay_amount = p["pay_amount"].as_f64().unwrap();
    assert_eq!(pay_amount, 1.0);

    // Customer pays; the worker detects and sweeps (deploying the wallet).
    let msg_hash = fund(&faucet, &deposit, 1_000_000_000).await;
    assert!(wait_balance(&faucet, &deposit, 1_000_000_000).await >= 1_000_000_000, "deposit funded");
    // A sweep recorded with the hash of its external message (not seen in time) is found by that hash.
    let check = SweepCheck {
        txid: msg_hash,
        from_address: faucet.address_from_secret(FAUCET_SECRET, WalletVersion::V3R2, 42, -1).unwrap(),
        to_address: deposit.clone(),
        broadcast_at: chrono::Utc::now(),
    };
    let mut state = SweepState::Pending;
    for _ in 0..20 {
        state = faucet.sweep_state(&check, &AssetContext::native(9)).await.unwrap();
        if state != SweepState::Pending {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    assert_eq!(state, SweepState::Confirmed);
    let unknown = SweepCheck { txid: "ab".repeat(32), broadcast_at: chrono::Utc::now() - chrono::Duration::minutes(11), ..check };
    assert!(matches!(faucet.sweep_state(&unknown, &AssetContext::native(9)).await.unwrap(), SweepState::Failed(_)));
    let (done, sweeps) = s.settle(&key, id, 120).await;
    assert_eq!(done["payment_status"], "finished", "expected finished: {done}");
    assert_eq!(sweeps.len(), 1);
    let outcome = done["outcome_amount"].as_f64().unwrap();
    assert_eq!(outcome, pay_amount);
    let net: f64 = sweeps[0].amount.to_string().parse().unwrap();
    assert!(net > pay_amount * 0.97 && net < pay_amount, "swept {net} for {pay_amount}");
    let merchant_got = wait_balance(&faucet, &merchant, (net * 0.999e9) as u64).await;
    assert!(merchant_got as f64 >= net * 0.999e9, "merchant received {merchant_got} nanoton for {net} swept");
    assert!(faucet.balance_nanoton(&deposit).await.unwrap() < 1_000_000, "deposit emptied");
    assert!(comments(&merchant).await.iter().any(|c| c == MEMO), "sweep carries the memo");
    println!("TON sweep {}: fee {} TON (deploy + transfer)", sweeps[0].txid.as_deref().unwrap_or(""), sweeps[0].network_fee_amount);
    assert_eq!(s.sweeps_final(id, 60).await[0].status, "confirmed");

    // A top-up after the sweep is credited and swept from the deployed wallet.
    fund(&faucet, &deposit, 1_000_000_000).await;
    let mut topped = Value::Null;
    for _ in 0..60 {
        let (p, sweeps) = s.settle(&key, id, 0).await;
        topped = p;
        if sweeps.len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let credited = topped["outcome_amount"].as_f64().unwrap() - outcome;
    assert!(credited > 0.97, "top-up credited: {topped}");
    let sweeps = s.worker.store.sweeps_for_payment(id).await.unwrap();
    assert_eq!(sweeps.len(), 2);
    println!("TON top-up sweep {}: fee {} TON", sweeps[1].txid.as_deref().unwrap_or(""), sweeps[1].network_fee_amount);
    assert!(
        wait_balance(&faucet, &merchant, merchant_got + 970_000_000).await >= merchant_got + 970_000_000,
        "top-up reached the merchant"
    );
}
