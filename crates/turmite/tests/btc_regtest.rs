//! Bitcoin on the local regtest node (deploy/testnets: bitcoind on 127.0.0.1:18443, Postgres on
//! 127.0.0.1:5433). The merchant's account key comes from a keyed regtest wallet. Deposits land
//! in that wallet; the engine only watches. Runs with `TURMITE_TEST_CHAINS=bitcoin`.

mod common;

use std::time::Duration;

use serde_json::{Value, json};

const RPC: &str = "http://127.0.0.1:18443";
const PAYER: &str = "turmite-test-payer";

/// The tests share one chain and mine blocks: they run one at a time.
static CHAIN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Node {
    http: reqwest::Client,
}

impl Node {
    async fn up() -> Option<Node> {
        let n = Node { http: reqwest::Client::new() };
        match n.try_call(None, "getblockchaininfo", json!([])).await {
            Ok(info) if info["chain"] == "regtest" => Some(n),
            _ => {
                eprintln!("SKIP: regtest bitcoind is not reachable on {RPC} (start deploy/testnets)");
                None
            }
        }
    }

    async fn try_call(&self, wallet: Option<&str>, method: &str, params: Value) -> Result<Value, Value> {
        let url = match wallet {
            Some(w) => format!("{RPC}/wallet/{w}"),
            None => RPC.to_string(),
        };
        let body = json!({ "jsonrpc": "2.0", "id": "t", "method": method, "params": params });
        let resp = self.http.post(url).basic_auth("devuser", Some("devpass")).json(&body).send().await.map_err(|e| json!(e.to_string()))?;
        let v: Value = resp.json().await.map_err(|e| json!(e.to_string()))?;
        match v.get("error") {
            Some(e) if !e.is_null() => Err(e.clone()),
            _ => Ok(v["result"].clone()),
        }
    }

    async fn call(&self, wallet: Option<&str>, method: &str, params: Value) -> Value {
        self.try_call(wallet, method, params).await.unwrap_or_else(|e| panic!("{method}: {e}"))
    }

    /// The shared payer wallet, with mature coins.
    async fn payer(&self) {
        for _ in 0..20 {
            let loaded = self.call(None, "listwallets", json!([])).await;
            if loaded.as_array().unwrap().iter().any(|w| w == PAYER) {
                break;
            }
            if self.try_call(None, "loadwallet", json!([PAYER])).await.is_err() {
                let _ = self.try_call(None, "createwallet", json!({ "wallet_name": PAYER })).await;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if self.call(Some(PAYER), "getbalance", json!([])).await.as_f64().unwrap_or(0.0) < 1.0 {
            self.mine(101).await;
        }
    }

    async fn mine(&self, blocks: u32) {
        let to = self.call(Some(PAYER), "getnewaddress", json!([])).await;
        self.call(None, "generatetoaddress", json!([blocks, to])).await;
    }

    /// A new keyed wallet for the merchant. Returns (wallet name, account tpub of its BIP84 account).
    async fn merchant(&self) -> (String, String) {
        let name = format!("merchant-{}", uuid::Uuid::new_v4().simple());
        self.call(None, "createwallet", json!({ "wallet_name": name })).await;
        let list = self.call(Some(&name), "listdescriptors", json!([])).await;
        let desc = list["descriptors"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|d| d["desc"].as_str().filter(|_| d["internal"] == false))
            .find(|d| d.starts_with("wpkh("))
            .expect("a wpkh receive descriptor")
            .to_string();
        let tpub = desc.split(']').nth(1).unwrap().split('/').next().unwrap().to_string();
        (name, tpub)
    }
}

/// An app on real regtest drivers built from a node row (exercises the registry), with a project.
async fn app() -> (common::TestApp, i64, String, i64) {
    // Engine logs show with a failing test.
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let t = common::start(common::TestOptions { drivers: None, ..Default::default() }).await;
    let (pid, key) = t.project().await;
    let net = t.app.store.list_networks().await.unwrap().into_iter().find(|n| n.chain == "bitcoin").unwrap();
    let sealed = t.app.sealer.seal_str("node_url", "http://devuser:devpass@127.0.0.1:18443");
    t.app.store.add_node(net.network_id, "regtest", &sealed, RPC, 0).await.unwrap();
    t.app.refresh_drivers().await.unwrap();
    let driver = t.app.drivers().get("bitcoin").expect("bitcoin driver from the node row");
    assert!(!driver.is_mock());
    assert!(!driver.needs_sweep());
    (t, pid, key, net.network_id)
}

/// A signed-in root user for the dashboard. Returns the session cookie.
async fn dashboard_session(t: &common::TestApp) -> String {
    let uid = t.app.store.create_user("root@example.com", "x", "root").await.unwrap();
    let token = format!("tok{}", uuid::Uuid::new_v4().simple());
    let hash = turmite::crypto::sha256_hex(token.as_bytes());
    t.app.store.create_session(&hash, uid, None, None, chrono::Utc::now() + chrono::Duration::hours(1)).await.unwrap();
    format!("turmite_session={token}")
}

async fn form(t: &common::TestApp, cookie: &str, path: &str, fields: &[(&str, &str)]) -> String {
    let r = t.client.post(t.url(path)).header("cookie", cookie).header("origin", &t.base).form(fields).send().await.unwrap();
    assert_eq!(r.status(), 303, "{path}");
    // The flash cookie carries the message.
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

async fn status(t: &common::TestApp, key: &str, id: i64) -> Value {
    t.get(&format!("/v1/payment/{id}"), key).await.1
}

/// Run the worker until the payment reaches `want` (the node wallet sees new transactions a moment late).
async fn tick_until(t: &common::TestApp, key: &str, id: i64, want: &str) -> Value {
    let mut p = Value::Null;
    for _ in 0..40 {
        t.tick().await;
        p = status(t, key, id).await;
        if p["payment_status"] == want {
            return p;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("payment {id} did not reach {want}: {p}");
}

async fn create(t: &common::TestApp, key: &str) -> Value {
    let (code, p) = t.post("/v1/payment", key, &json!({ "price_amount": 100, "price_currency": "usd", "pay_currency": "btc" })).await;
    assert_eq!(code, 201, "{p}");
    p
}

#[tokio::test]
async fn btc_watch_only_payment_lands_in_the_merchant_wallet() {
    if !common::chain_enabled("bitcoin") {
        return;
    }
    let _chain = CHAIN.lock().await;
    let Some(node) = Node::up().await else { return };
    node.payer().await;
    let (merchant, tpub) = node.merchant().await;
    let (t, pid, key, nid) = app().await;

    // The operator saves the key as a vpub (SLIP-132) on the Wallets page.
    let vpub = {
        let tpub_raw = bs58_check_decode(&tpub);
        let mut raw = tpub_raw.clone();
        raw[..4].copy_from_slice(&[0x04, 0x5f, 0x1c, 0xf6]);
        bs58_check_encode(&raw)
    };
    let cookie = dashboard_session(&t).await;
    let msg = form(&t, &cookie, &format!("/dashboard/p/{pid}/wallets/{nid}/bitcoin"), &[("xpub", "tprv-not-a-key"), ("totp", "")]).await;
    assert!(msg.contains("not a valid"), "{msg}");
    let msg = form(&t, &cookie, &format!("/dashboard/p/{pid}/wallets/{nid}/bitcoin"), &[("xpub", &vpub), ("totp", "")]).await;
    assert!(msg.contains("account key saved"), "{msg}");
    let w = t.app.store.project_wallet(pid, nid).await.unwrap().unwrap();
    assert_eq!(w.xpub.as_deref(), Some(tpub.as_str()), "stored as tpub");

    // The page shows the first receive addresses; the merchant's wallet hands out the same ones.
    let page =
        t.client.get(t.url(&format!("/dashboard/p/{pid}/wallets"))).header("cookie", &cookie).send().await.unwrap().text().await.unwrap();
    let mut own = Vec::new();
    for _ in 0..3 {
        own.push(node.call(Some(&merchant), "getnewaddress", json!(["", "bech32"])).await.as_str().unwrap().to_string());
    }
    for a in &own {
        assert!(page.contains(a.as_str()), "wallets page shows {a}");
    }

    // A payment through the API gets receive address 0 of the merchant's account.
    let p = create(&t, &key).await;
    let id = common::pid(&p);
    let address = p["pay_address"].as_str().unwrap().to_string();
    assert_eq!(address, own[0]);
    let amount = format!("{:.8}", p["pay_amount"].as_f64().unwrap());

    node.call(Some(PAYER), "sendtoaddress", json!([address, amount])).await;
    let seen = tick_until(&t, &key, id, "confirming").await;
    assert_eq!(seen["actually_paid"], p["pay_amount"]);

    node.mine(1).await;
    let done = tick_until(&t, &key, id, "finished").await;
    assert_eq!(done["outcome_amount"], p["pay_amount"]);

    // No sweep: the coins are in the merchant's own wallet.
    assert!(t.app.store.sweeps_for_payment(id).await.unwrap().is_empty());
    let mut got = String::new();
    for _ in 0..20 {
        got = format!("{:.8}", node.call(Some(&merchant), "getreceivedbyaddress", json!([address, 1])).await.as_f64().unwrap());
        if got == amount {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(got, amount, "the merchant's wallet received the payment");

    // The next payment gets the next receive address.
    let p2 = create(&t, &key).await;
    assert_eq!(p2["pay_address"].as_str(), Some(own[1].as_str()));

    let _ = node.try_call(None, "unloadwallet", json!([merchant])).await;
}

#[tokio::test]
async fn btc_replaced_deposit_stops_counting() {
    if !common::chain_enabled("bitcoin") {
        return;
    }
    let _chain = CHAIN.lock().await;
    let Some(node) = Node::up().await else { return };
    node.payer().await;
    let (merchant, tpub) = node.merchant().await;
    let (t, pid, key, nid) = app().await;
    let driver = t.app.drivers().get("bitcoin").unwrap();
    let w = turmite::chains::driver::WalletContext { project_id: pid, xpub: Some(tpub.clone()), ..Default::default() };
    assert_eq!(driver.prepare_wallet(&w).await.unwrap().len(), 3);
    t.app.store.set_project_wallet_keys(pid, nid, Some(&tpub), None, None, None).await.unwrap();

    // Case 1: the replacement waits in the mempool, then confirms.
    let p = create(&t, &key).await;
    let id = common::pid(&p);
    let txid = pay_replaceable(&node, p["pay_address"].as_str().unwrap(), &p).await;
    tick_until(&t, &key, id, "confirming").await;
    replace(&node, &txid).await;
    let back = tick_until(&t, &key, id, "waiting").await;
    assert_eq!(back["actually_paid"], json!(0));
    assert!(dropped(&t, id).await, "the replaced deposit is marked dropped");
    node.mine(1).await;
    t.tick().await;
    assert_eq!(status(&t, &key, id).await["payment_status"], "waiting");

    // Case 2: the replacement is mined before the next watch; the wallet stops listing the deposit.
    let p = create(&t, &key).await;
    let id = common::pid(&p);
    let txid = pay_replaceable(&node, p["pay_address"].as_str().unwrap(), &p).await;
    tick_until(&t, &key, id, "confirming").await;
    replace(&node, &txid).await;
    node.mine(1).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    tick_until(&t, &key, id, "waiting").await;
    assert!(dropped(&t, id).await);

    let _ = node.try_call(None, "unloadwallet", json!([merchant])).await;
}

async fn pay_replaceable(node: &Node, address: &str, p: &Value) -> String {
    let amount = format!("{:.8}", p["pay_amount"].as_f64().unwrap());
    node.call(Some(PAYER), "sendtoaddress", json!({ "address": address, "amount": amount, "replaceable": true }))
        .await
        .as_str()
        .unwrap()
        .to_string()
}

/// Double-spend `txid`'s inputs to the payer itself with a higher fee.
async fn replace(node: &Node, txid: &str) {
    let tx = node.call(None, "getrawtransaction", json!([txid, true])).await;
    let mut inputs = Vec::new();
    let mut total = 0.0;
    for i in tx["vin"].as_array().unwrap() {
        let prev = node.call(None, "getrawtransaction", json!([i["txid"], true])).await;
        total += prev["vout"][i["vout"].as_u64().unwrap() as usize]["value"].as_f64().unwrap();
        inputs.push(json!({ "txid": i["txid"], "vout": i["vout"] }));
    }
    // The replacement must pay more fee than the original.
    let spent: f64 = tx["vout"].as_array().unwrap().iter().map(|o| o["value"].as_f64().unwrap()).sum();
    let fee = (total - spent) * 2.0 + 0.0001;
    let to = node.call(Some(PAYER), "getnewaddress", json!([])).await;
    let mut output = serde_json::Map::new();
    output.insert(to.as_str().unwrap().to_string(), json!(format!("{:.8}", total - fee)));
    let out = json!([output]);
    let raw = node.call(None, "createrawtransaction", json!([inputs, out, 0, true])).await;
    let signed = node.call(Some(PAYER), "signrawtransactionwithwallet", json!([raw])).await;
    node.call(None, "sendrawtransaction", json!([signed["hex"]])).await;
}

async fn dropped(t: &common::TestApp, payment_id: i64) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT bool_or(dropped_at IS NOT NULL) FROM payment_transactions WHERE payment_id = $1")
        .bind(payment_id)
        .fetch_one(&t.app.store.pool)
        .await
        .unwrap()
}

// Base58check for the test's SLIP-132 conversion.
const B58: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

fn bs58_check_decode(s: &str) -> Vec<u8> {
    let mut bytes: Vec<u8> = Vec::new();
    for c in s.bytes() {
        let mut carry = B58.iter().position(|&b| b == c).unwrap() as u32;
        for b in bytes.iter_mut().rev() {
            carry += (*b as u32) * 58;
            *b = carry as u8;
            carry >>= 8;
        }
        while carry > 0 {
            bytes.insert(0, carry as u8);
            carry >>= 8;
        }
    }
    bytes.truncate(bytes.len() - 4);
    bytes
}

fn bs58_check_encode(payload: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut data = payload.to_vec();
    data.extend_from_slice(&Sha256::digest(Sha256::digest(payload))[..4]);
    let mut digits: Vec<u8> = Vec::new();
    for &byte in &data {
        let mut carry = byte as u32;
        for d in digits.iter_mut().rev() {
            carry += (*d as u32) << 8;
            *d = (carry % 58) as u8;
            carry /= 58;
        }
        while carry > 0 {
            digits.insert(0, (carry % 58) as u8);
            carry /= 58;
        }
    }
    digits.iter().map(|&d| B58[d as usize] as char).collect()
}
