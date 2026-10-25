//! Monero on the local regtest stack (deploy/testnets: monerod 127.0.0.1:18081, engine
//! wallet-rpc 127.0.0.1:18083, payer wallet-rpc 127.0.0.1:18084, Postgres 127.0.0.1:5433).
//! The merchant gives an address and a view key; the engine makes a view-only wallet, a
//! customer pays a subaddress, and the payment finishes after 10 blocks. Runs with
//! `TURMITE_TEST_CHAINS=monero`.

mod common;

use std::time::Duration;

use serde_json::{Value, json};

const DAEMON: &str = "http://127.0.0.1:18081";
const ENGINE: &str = "http://127.0.0.1:18083";
const PAYER_RPC: &str = "http://127.0.0.1:18084";
const PAYER: &str = "turmite-test-payer";

async fn try_rpc(http: &reqwest::Client, url: &str, method: &str, params: Value) -> Result<Value, Value> {
    let body = json!({ "jsonrpc": "2.0", "id": "0", "method": method, "params": params });
    let v: Value = http
        .post(format!("{url}/json_rpc"))
        .json(&body)
        .send()
        .await
        .map_err(|e| json!(e.to_string()))?
        .json()
        .await
        .map_err(|e| json!(e.to_string()))?;
    match v.get("error") {
        Some(e) if !e.is_null() => Err(e.clone()),
        _ => Ok(v["result"].clone()),
    }
}

async fn rpc(http: &reqwest::Client, url: &str, method: &str, params: Value) -> Value {
    try_rpc(http, url, method, params).await.unwrap_or_else(|e| panic!("{method}: {e}"))
}

async fn open(http: &reqwest::Client, name: &str) {
    if try_rpc(http, PAYER_RPC, "open_wallet", json!({ "filename": name, "password": "" })).await.is_err() {
        rpc(http, PAYER_RPC, "create_wallet", json!({ "filename": name, "password": "", "language": "English" })).await;
    }
}

async fn address(http: &reqwest::Client) -> String {
    rpc(http, PAYER_RPC, "get_address", json!({ "account_index": 0 })).await["address"].as_str().unwrap().to_string()
}

async fn mine(http: &reqwest::Client, blocks: u64, to: &str) {
    rpc(http, DAEMON, "generateblocks", json!({ "amount_of_blocks": blocks, "wallet_address": to })).await;
}

async fn height(http: &reqwest::Client) -> i64 {
    let info: Value = http.get(format!("{DAEMON}/get_info")).send().await.unwrap().json().await.unwrap();
    info["height"].as_i64().unwrap()
}

/// Open the payer wallet with at least `piconero` unlocked. Returns its address.
async fn funded_payer(http: &reqwest::Client, piconero: u64) -> String {
    open(http, PAYER).await;
    let payer = address(http).await;
    for attempt in 0..5 {
        // A wallet out of step with the daemon does not see new outputs: rescan once.
        if attempt == 2 {
            rpc(http, PAYER_RPC, "rescan_blockchain", json!({})).await;
        }
        rpc(http, PAYER_RPC, "refresh", json!({})).await;
        let b = rpc(http, PAYER_RPC, "get_balance", json!({ "account_index": 0 })).await;
        if b["unlocked_balance"].as_u64().unwrap_or(0) >= piconero {
            return payer;
        }
        // Coinbase outputs unlock after 60 blocks.
        mine(http, 80, &payer).await;
    }
    panic!("the payer wallet could not be funded");
}

async fn status(t: &common::TestApp, key: &str, id: i64) -> Value {
    t.get(&format!("/v1/payment/{id}"), key).await.1
}

async fn tick_until(t: &common::TestApp, key: &str, id: i64, want: &str) -> Value {
    let mut p = Value::Null;
    for _ in 0..40 {
        t.tick().await;
        p = status(t, key, id).await;
        if p["payment_status"] == want {
            return p;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("payment {id} did not reach {want}: {p}");
}

#[tokio::test]
async fn xmr_view_only_payment_lands_in_the_merchant_wallet() {
    if !common::chain_enabled("monero") {
        return;
    }
    let http = reqwest::Client::new();
    for url in [ENGINE, PAYER_RPC] {
        if try_rpc(&http, url, "get_version", json!({})).await.is_err() {
            eprintln!("SKIP: monero wallet-rpc on {url} is not reachable (start deploy/testnets)");
            return;
        }
    }

    // The merchant's wallet lives on the payer RPC; the engine only gets its address and view key.
    let merchant = format!("merchant-{}", uuid::Uuid::new_v4().simple());
    open(&http, &merchant).await;
    let merchant_address = address(&http).await;
    let view_key = rpc(&http, PAYER_RPC, "query_key", json!({ "key_type": "view_key" })).await["key"].as_str().unwrap().to_string();
    let payer = funded_payer(&http, 5_000_000_000_000).await;
    let other_view_key = rpc(&http, PAYER_RPC, "query_key", json!({ "key_type": "view_key" })).await["key"].as_str().unwrap().to_string();
    let restore_height = height(&http).await - 1;

    // Real driver from a node row.
    let t = common::start(common::TestOptions { drivers: None, ..Default::default() }).await;
    let (pid, key) = t.project().await;
    let net = t.app.store.list_networks().await.unwrap().into_iter().find(|n| n.chain == "monero").unwrap();
    let sealed = t.app.sealer.seal_str("node_url", ENGINE);
    t.app.store.add_node(net.network_id, "regtest", &sealed, ENGINE, 0).await.unwrap();
    t.app.refresh_drivers().await.unwrap();
    let driver = t.app.drivers().get("monero").expect("monero driver from the node row");
    assert!(!driver.is_mock() && !driver.needs_sweep());

    // A view key of another wallet is refused before any wallet file is made.
    let wrong = turmite::chains::driver::WalletContext {
        project_id: pid,
        address: Some(merchant_address.clone()),
        view_key: Some(zeroize::Zeroizing::new(other_view_key)),
        restore_height: Some(restore_height),
        ..Default::default()
    };
    let err = driver.prepare_wallet(&wrong).await.unwrap_err();
    assert!(err.to_string().contains("does not belong"), "{err}");

    // The operator saves the view-only wallet on the Wallets page.
    let cookie = dashboard_session(&t).await;
    let path = format!("/dashboard/p/{pid}/wallets/{}/monero", net.network_id);
    let h = restore_height.to_string();
    let msg =
        form(&t, &cookie, &path, &[("address", &merchant_address), ("view_key", &view_key), ("restore_height", &h), ("totp", "")]).await;
    assert!(msg.contains("view-only wallet saved"), "{msg}");
    let w = turmite::addresses::wallet_context(&t.app, pid, net.network_id).await.unwrap();
    assert_eq!(w.view_key.as_deref().map(String::as_str), Some(view_key.as_str()), "view key sealed and readable");
    assert_eq!(w.restore_height, Some(restore_height));
    let page =
        t.client.get(t.url(&format!("/dashboard/p/{pid}/wallets"))).header("cookie", &cookie).send().await.unwrap().text().await.unwrap();
    assert!(page.contains(&merchant_address) && !page.contains(&view_key), "address shown, view key never shown");

    // A payment gets a subaddress of the merchant's wallet, with the index the wallet gave.
    let (code, p) = t.post("/v1/payment", &key, &json!({ "price_amount": 100, "price_currency": "usd", "pay_currency": "xmr" })).await;
    assert_eq!(code, 201, "{p}");
    let id = common::pid(&p);
    let deposit = p["pay_address"].as_str().unwrap().to_string();
    assert!(turmite::chains::xmr::is_subaddress_of(&merchant_address, &view_key, &deposit), "{deposit}");
    let (index, _) = t.app.store.deposit_address_for_payment(id).await.unwrap().unwrap();
    assert!(index >= 1, "subaddress index from the wallet");

    let piconero = (p["pay_amount"].as_f64().unwrap() * 1e12).round() as u64;
    rpc(&http, PAYER_RPC, "transfer", json!({ "destinations": [{ "amount": piconero, "address": deposit }], "account_index": 0 })).await;
    tick_until(&t, &key, id, "confirming").await;

    // Outputs unlock after 10 blocks; the network needs 10 confirmations.
    mine(&http, 5, &payer).await;
    t.tick().await;
    assert_eq!(status(&t, &key, id).await["payment_status"], "confirming");
    mine(&http, 6, &payer).await;
    let done = tick_until(&t, &key, id, "finished").await;
    assert_eq!(done["outcome_amount"], p["pay_amount"]);
    assert!(t.app.store.sweeps_for_payment(id).await.unwrap().is_empty(), "no sweep");

    // The merchant's own wallet received the coins.
    open(&http, &merchant).await;
    rpc(&http, PAYER_RPC, "refresh", json!({})).await;
    let r = rpc(&http, PAYER_RPC, "get_transfers", json!({ "in": true, "account_index": 0 })).await;
    let got: u64 = r["in"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["address"].as_str() == Some(deposit.as_str()))
        .map(|t| t["amount"].as_u64().unwrap_or(0))
        .sum();
    assert_eq!(got, piconero);
    rpc(&http, PAYER_RPC, "close_wallet", json!({})).await;
}

async fn dashboard_session(t: &common::TestApp) -> String {
    let uid = t.app.store.create_user("root@example.com", "x", "root").await.unwrap();
    let token = format!("tok{}", uuid::Uuid::new_v4().simple());
    let hash = turmite::crypto::sha256_hex(token.as_bytes());
    t.app.store.create_session(&hash, uid, None, None, chrono::Utc::now() + chrono::Duration::hours(1)).await.unwrap();
    format!("turmite_session={token}")
}

/// Post a dashboard form and return the flash message.
async fn form(t: &common::TestApp, cookie: &str, path: &str, fields: &[(&str, &str)]) -> String {
    let r = t.client.post(t.url(path)).header("cookie", cookie).header("origin", &t.base).form(fields).send().await.unwrap();
    assert_eq!(r.status(), 303, "{path}");
    let flash = r
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| c.strip_prefix("turmite_flash="))
        .map(|c| c.split(';').next().unwrap_or_default().to_string())
        .unwrap_or_default();
    url::form_urlencoded::parse(format!("x={flash}").as_bytes()).next().map(|(_, v)| v.into_owned()).unwrap_or_default()
}
