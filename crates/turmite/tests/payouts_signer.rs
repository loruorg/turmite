//! The signer's state machine against a scripted EVM node: fee bump, refused broadcast, unknown outcome.
#![cfg(feature = "evm")]

mod common;

use std::sync::{Arc, Mutex};

use axum::{Json, extract::State};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use turmite::{
    hot,
    payouts::{
        PayoutStatus,
        limits::PayoutLimits,
        service::{self, NewWithdrawal, Source},
        signer,
    },
};

#[derive(Default)]
struct Node {
    /// Raw transactions the node took, with their hashes.
    sent: Vec<(String, String)>,
    /// Refuse every broadcast with this message.
    refuse: Option<String>,
    /// Hashes in a block (receipt status 1).
    mined: Vec<String>,
    /// Hashes the node no longer knows (dropped from its mempool).
    dropped: Vec<String>,
    /// `eth_getTransactionCount(.., "latest")`.
    latest_nonce: u64,
    head: u64,
}

type Shared = Arc<Mutex<Node>>;

async fn handle(State(node): State<Shared>, Json(req): Json<Value>) -> Json<Value> {
    let mut n = node.lock().unwrap();
    let p = &req["params"];
    let result: Result<Value, String> = match req["method"].as_str().unwrap_or("") {
        "eth_chainId" => Ok(json!("0x7a69")),
        "eth_getBlockByNumber" => Ok(json!({ "number": format!("{:#x}", n.head), "baseFeePerGas": "0x3b9aca00" })),
        "eth_maxPriorityFeePerGas" => Ok(json!("0x3b9aca00")),
        "eth_estimateGas" => Ok(json!("0x5208")),
        "eth_getBalance" => Ok(json!("0x56bc75e2d63100000")),
        "eth_blockNumber" => Ok(json!(format!("{:#x}", n.head))),
        "eth_getTransactionCount" => {
            if p[1] == "latest" {
                Ok(json!(format!("{:#x}", n.latest_nonce)))
            } else {
                Ok(json!("0x0"))
            }
        }
        "eth_sendRawTransaction" => match &n.refuse {
            Some(msg) => Err(msg.clone()),
            None => {
                let raw = p[0].as_str().unwrap().to_string();
                let hash = format!("{:#x}", alloy::primitives::keccak256(hex::decode(&raw[2..]).unwrap()));
                n.sent.push((raw, hash.clone()));
                Ok(json!(hash))
            }
        },
        "eth_getTransactionReceipt" => {
            let h = p[0].as_str().unwrap().to_string();
            Ok(if n.mined.contains(&h) {
                json!({ "transactionHash": h, "blockNumber": "0x64", "status": "0x1", "gasUsed": "0x5208", "effectiveGasPrice": "0x77359400" })
            } else {
                Value::Null
            })
        }
        "eth_getTransactionByHash" => {
            let h = p[0].as_str().unwrap().to_string();
            Ok(if n.sent.iter().any(|(_, x)| *x == h) && !n.dropped.contains(&h) { json!({ "hash": h }) } else { Value::Null })
        }
        other => Err(format!("method {other} not scripted")),
    };
    Json(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": req["id"], "result": r }),
        Err(m) => json!({ "jsonrpc": "2.0", "id": req["id"], "error": { "code": -32000, "message": m } }),
    })
}

async fn fake_node() -> (String, Shared) {
    let node: Shared = Arc::new(Mutex::new(Node { head: 100, ..Default::default() }));
    let app = axum::Router::new().route("/", axum::routing::post(handle)).with_state(node.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, node)
}

async fn setup() -> (common::TestApp, Shared, i64, i64) {
    let t = common::start(Default::default()).await;
    let (url, node) = fake_node().await;
    let (pid, _) = t.project().await;
    let uid = t.app.store.create_user("ops@shop.test", "x", "member").await.unwrap();
    let s = hot::generate_secret("evm").unwrap();
    let address = hot::derive_address("evm", &s, true).unwrap();
    let id = hot::store(&t.app, "evm", "hot", &address, &s, None).await.unwrap();
    hot::assign(&t.app, pid, "evm", id).await.unwrap();
    sqlx::query("UPDATE network SET chain_id = 31337, confirmations_required = 2 WHERE code = 'ETH'")
        .execute(&t.app.store.pool)
        .await
        .unwrap();
    let eth = t.app.store.network_by_chain("ethereum").await.unwrap().unwrap();
    t.app.store.add_node(eth.network_id, "fake", &t.app.sealer.seal_str("node_url", &url), &url, 0).await.unwrap();
    let l = PayoutLimits { auto_approve_usd: Decimal::from(1_000_000), ..Default::default() };
    t.app.store.set_payout_limits(pid, &l).await.unwrap();
    (t, node, pid, uid)
}

async fn new_payout(t: &common::TestApp, pid: i64, uid: i64) -> i64 {
    let w = NewWithdrawal {
        address: "0x70997970C51812dc3A010C7d01b50e0d17dc79C8".into(),
        currency: "eth".into(),
        amount: "0.1".parse().unwrap(),
        ..Default::default()
    };
    service::create_batch(&t.app, pid, Source::Dashboard { user_id: uid }, vec![w], None).await.unwrap().payouts[0].payout_id
}

async fn tick(t: &common::TestApp, id: i64) -> turmite::db::PayoutRow {
    sqlx::query("UPDATE payouts SET next_check_at = now() WHERE payout_id = $1").bind(id).execute(&t.app.store.pool).await.unwrap();
    signer::tick(&t.app).await;
    t.app.store.payout_any(id).await.unwrap().unwrap()
}

async fn sql(t: &common::TestApp, q: &str, id: i64) {
    sqlx::query(sqlx::AssertSqlSafe(q.to_string())).bind(id).execute(&t.app.store.pool).await.unwrap();
}

#[tokio::test]
async fn stuck_transaction_is_bumped_with_the_same_nonce() {
    let (t, node, pid, uid) = setup().await;
    let id = new_payout(&t, pid, uid).await;
    let p = tick(&t, id).await;
    assert_eq!(p.status, PayoutStatus::Initiated);
    let first = t.app.store.payout_attempts(id).await.unwrap();
    assert_eq!(first.len(), 1);

    // Still in the mempool after the bump delay: a second transaction, same nonce, higher fees.
    sql(&t, "UPDATE payout_attempts SET created_at = now() - interval '10 minutes' WHERE payout_id = $1", id).await;
    tick(&t, id).await;
    let attempts = t.app.store.payout_attempts(id).await.unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].nonce, attempts[1].nonce);
    let fee = |a: &turmite::db::AttemptRow| a.params["max_fee"].as_str().unwrap().parse::<u128>().unwrap();
    assert!(fee(&attempts[1]) * 100 >= fee(&attempts[0]) * 125, "{} -> {}", fee(&attempts[0]), fee(&attempts[1]));
    assert_eq!(node.lock().unwrap().sent.len(), 2);

    // The bumped one is mined; it needs 2 confirmations.
    let bumped = attempts[1].txid.clone();
    node.lock().unwrap().mined.push(bumped.clone());
    let p = tick(&t, id).await;
    assert_eq!((p.status, p.confirmations), (PayoutStatus::Initiated, 1));
    node.lock().unwrap().head += 1;
    let p = tick(&t, id).await;
    assert_eq!(p.status, PayoutStatus::Sent);
    assert_eq!(p.txid.as_deref(), Some(bumped.as_str()));
    assert_eq!(p.fee, Some("0.000042".parse().unwrap()));
}

#[tokio::test]
async fn refused_broadcast_fails_after_two_minutes_only() {
    let (t, node, pid, uid) = setup().await;
    node.lock().unwrap().refuse = Some("insufficient funds for gas * price + value".into());
    let id = new_payout(&t, pid, uid).await;
    let p = tick(&t, id).await;
    assert_eq!(p.status, PayoutStatus::Signing, "stored, not failed yet");
    assert!(p.refused_at.is_some());
    assert!(p.error.as_deref().unwrap().contains("insufficient funds"));
    assert_eq!(t.app.store.payout_attempts(id).await.unwrap().len(), 1);
    // Still refused a minute later: still waiting.
    sql(&t, "UPDATE payouts SET refused_at = now() - interval '1 minute' WHERE payout_id = $1", id).await;
    assert_eq!(tick(&t, id).await.status, PayoutStatus::Signing);
    // After 2 minutes, unknown to every node: failed, final.
    sql(&t, "UPDATE payouts SET refused_at = now() - interval '3 minutes' WHERE payout_id = $1", id).await;
    let p = tick(&t, id).await;
    assert_eq!(p.status, PayoutStatus::Failed);
    assert!(p.error.as_deref().unwrap().contains("Every node refused"));

    // A refused payout that a node knows after all is tracked, not failed.
    let id = new_payout(&t, pid, uid).await;
    tick(&t, id).await;
    let a = t.app.store.payout_attempts(id).await.unwrap().remove(0);
    {
        let mut n = node.lock().unwrap();
        n.sent.push((a.raw_tx.clone(), a.txid.clone()));
        n.refuse = Some("nonce too low".into());
    }
    sql(&t, "UPDATE payouts SET refused_at = now() - interval '3 minutes' WHERE payout_id = $1", id).await;
    assert_eq!(tick(&t, id).await.status, PayoutStatus::Initiated);
}

#[tokio::test]
async fn used_nonce_without_our_transaction_needs_review() {
    let (t, node, pid, uid) = setup().await;
    let id = new_payout(&t, pid, uid).await;
    assert_eq!(tick(&t, id).await.status, PayoutStatus::Initiated);
    let a = t.app.store.payout_attempts(id).await.unwrap().remove(0);
    {
        let mut n = node.lock().unwrap();
        n.dropped.push(a.txid.clone());
        n.latest_nonce = 1;
    }
    // Under 30 minutes: wait.
    assert_eq!(tick(&t, id).await.status, PayoutStatus::Initiated);
    sql(&t, "UPDATE payout_attempts SET broadcast_at = now() - interval '31 minutes' WHERE payout_id = $1", id).await;
    let p = tick(&t, id).await;
    assert_eq!(p.status, PayoutStatus::NeedsReview);
    let e = service::resolve(&t.app, pid, id, true, "Found on the explorer under another hash.").await.unwrap();
    assert_eq!(e.status, PayoutStatus::Sent);

    // Dropped but the nonce is free: broadcast again, no new signature.
    let id = new_payout(&t, pid, uid).await;
    node.lock().unwrap().latest_nonce = 0;
    tick(&t, id).await;
    let a = t.app.store.payout_attempts(id).await.unwrap().remove(0);
    node.lock().unwrap().dropped.push(a.txid.clone());
    let before = node.lock().unwrap().sent.len();
    tick(&t, id).await;
    assert_eq!(node.lock().unwrap().sent.len(), before + 1, "the stored transaction went out again");
    assert_eq!(t.app.store.payout_attempts(id).await.unwrap().len(), 1);
}
