//! Sweeps on mock chains: confirmation on chain, a new try after a failure, old payments that
//! still hold funds, and the dashboard warning.

mod common;

use reqwest::{Client, header};
use rust_decimal::Decimal;
use serde_json::json;
use turmite::chains::driver::{SweepState, as_mock};

/// A Tron payment of 10 TRX with a confirmed deposit. Returns (payment id, api key).
async fn paid_trx(t: &common::TestApp, key: &str) -> i64 {
    let (status, p) = t.post("/v1/payment", key, &json!({ "price_amount": 10, "price_currency": "usd", "pay_currency": "trx" })).await;
    assert_eq!(status, 201, "{p}");
    let id = common::pid(&p);
    t.deposit(id, "10", 100).await;
    id
}

fn set_state(t: &common::TestApp, txid: &str, state: SweepState) {
    let driver = t.app.drivers().get("tron").unwrap();
    as_mock(&driver).unwrap().set_sweep_state(txid, state);
}

/// (status, txid) of each sweep of a payment.
async fn sweeps(t: &common::TestApp, id: i64) -> Vec<(String, String)> {
    t.app.store.sweeps_for_payment(id).await.unwrap().into_iter().map(|s| (s.status, s.txid.unwrap_or_default())).collect()
}

async fn swept_gross(t: &common::TestApp, id: i64) -> Decimal {
    sqlx::query_scalar("SELECT swept_gross FROM payments WHERE payment_id = $1").bind(id).fetch_one(&t.app.store.pool).await.unwrap()
}

#[tokio::test]
async fn a_failed_sweep_is_sent_again_and_a_confirmed_one_is_final() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let id = paid_trx(&t, &key).await;
    let first = sweeps(&t, id).await;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].0, "broadcast");
    let tx1 = first[0].1.clone();

    // Not final yet: nothing is sent again.
    set_state(&t, &tx1, SweepState::Pending);
    turmite::worker::process_payment(&t.app, id).await.unwrap();
    assert_eq!(sweeps(&t, id).await, [("broadcast".to_string(), tx1.clone())]);
    assert_eq!(swept_gross(&t, id).await, Decimal::from(10));

    // It failed on chain: the amount goes back and a new sweep sends it.
    set_state(&t, &tx1, SweepState::Failed("test failure".into()));
    turmite::worker::process_payment(&t.app, id).await.unwrap();
    let now = sweeps(&t, id).await;
    assert_eq!(now.len(), 2, "{now:?}");
    assert_eq!(now[0], ("failed".to_string(), tx1.clone()));
    assert_eq!(now[1].0, "broadcast");
    assert_eq!(swept_gross(&t, id).await, Decimal::from(10), "reserved once, not twice");
    let failed = &t.app.store.sweeps_for_payment(id).await.unwrap()[0];
    assert_eq!(failed.last_error.as_deref(), Some("test failure"));

    // The new one is final: no other sweep.
    turmite::worker::process_payment(&t.app, id).await.unwrap();
    turmite::worker::process_payment(&t.app, id).await.unwrap();
    let done = sweeps(&t, id).await;
    assert_eq!(done.len(), 2);
    assert_eq!(done[1].0, "confirmed");
}

#[tokio::test]
async fn repeated_failures_wait_before_the_next_try() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let id = paid_trx(&t, &key).await;
    for n in 1..=turmite::worker::QUICK_SWEEP_RETRIES as usize {
        let all = sweeps(&t, id).await;
        assert_eq!(all.len(), n, "{all:?}");
        set_state(&t, &all[n - 1].1, SweepState::Failed("out of energy".into()));
        turmite::worker::process_payment(&t.app, id).await.unwrap();
    }
    let all = sweeps(&t, id).await;
    assert_eq!(all.len(), 3, "no fourth try at once: {all:?}");
    assert!(all.iter().all(|s| s.0 == "failed"));
    assert_eq!(swept_gross(&t, id).await, Decimal::ZERO);

    // An hour later the worker tries again.
    sqlx::query("UPDATE sweeps SET updated_at = now() - interval '61 minutes' WHERE payment_id = $1")
        .bind(id)
        .execute(&t.app.store.pool)
        .await
        .unwrap();
    turmite::worker::process_payment(&t.app, id).await.unwrap();
    assert_eq!(sweeps(&t, id).await.len(), 4);
}

#[tokio::test]
async fn a_refused_sweep_counts_toward_the_retry_limit() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let driver = t.app.drivers().get("tron").unwrap();
    as_mock(&driver).unwrap().fail_sweeps(Some("out of energy"));
    let id = paid_trx(&t, &key).await;
    for _ in 0..5 {
        turmite::worker::process_payment(&t.app, id).await.unwrap();
    }
    let all = t.app.store.sweeps_for_payment(id).await.unwrap();
    assert_eq!(all.len(), turmite::worker::QUICK_SWEEP_RETRIES as usize, "then it waits");
    assert!(all.iter().all(|s| s.status == "failed" && s.txid.is_none()));
    assert!(all[0].last_error.as_deref().unwrap_or_default().contains("out of energy"));
    assert_eq!(swept_gross(&t, id).await, Decimal::ZERO);

    // After the wait, the next try goes out.
    sqlx::query("UPDATE sweeps SET updated_at = now() - interval '2 hours' WHERE payment_id = $1")
        .bind(id)
        .execute(&t.app.store.pool)
        .await
        .unwrap();
    as_mock(&driver).unwrap().fail_sweeps(None);
    turmite::worker::process_payment(&t.app, id).await.unwrap();
    let last = sweeps(&t, id).await.pop().unwrap();
    assert_eq!(last.0, "broadcast");
    assert_eq!(swept_gross(&t, id).await, Decimal::from(10));
}

#[tokio::test]
async fn old_payments_that_still_hold_funds_are_swept() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let id = paid_trx(&t, &key).await;
    let tx1 = sweeps(&t, id).await[0].1.clone();
    // The payment leaves the monitoring window while its sweep is open.
    sqlx::query("UPDATE payments SET expires_at = now() - interval '2 days' WHERE payment_id = $1")
        .bind(id)
        .execute(&t.app.store.pool)
        .await
        .unwrap();
    assert!(!t.app.store.active_payment_ids().await.unwrap().contains(&id));
    assert_eq!(t.app.store.sweep_backlog_ids(&["tron"]).await.unwrap(), [id], "an open sweep keeps it in the backlog");

    // The sweep fails: the backlog pass sends the funds again, then confirms the new sweep.
    set_state(&t, &tx1, SweepState::Failed("dropped".into()));
    turmite::worker::sweep_backlog(&t.app).await;
    let now = sweeps(&t, id).await;
    assert_eq!(now.len(), 2, "{now:?}");
    assert_eq!((now[0].0.as_str(), now[1].0.as_str()), ("failed", "broadcast"));
    turmite::worker::sweep_backlog(&t.app).await;
    assert_eq!(sweeps(&t, id).await[1].0, "confirmed");
    assert!(t.app.store.sweep_backlog_ids(&["tron"]).await.unwrap().is_empty(), "nothing left to sweep");
}

fn browser() -> Client {
    Client::builder().cookie_store(true).redirect(reqwest::redirect::Policy::none()).build().unwrap()
}

#[tokio::test]
async fn the_dashboard_shows_a_stuck_sweep() {
    let t = common::start(Default::default()).await;
    let c = browser();
    let r = c
        .post(t.url("/dashboard/setup"))
        .header(header::ORIGIN, &t.base)
        .form(&[("email", "root@example.com"), ("password", "a very long password"), ("password2", "a very long password")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    let pid = t.app.store.list_projects().await.unwrap()[0].project_id;
    for n in t.app.store.list_networks().await.unwrap() {
        t.app.store.upsert_project_wallet_address(pid, n.network_id, &format!("test-{}-wallet", n.chain), None).await.unwrap();
    }
    let (key, _) = t.app.store.create_api_key(pid, "test", None).await.unwrap();
    let id = paid_trx(&t, &key).await;
    let overview = format!("/dashboard/p/{pid}");
    let detail = format!("/dashboard/p/{pid}/payments/{id}");
    let page = |path: String| {
        let c = c.clone();
        let url = t.url(&path);
        async move { c.get(url).send().await.unwrap().text().await.unwrap() }
    };
    assert!(!page(overview.clone()).await.contains("Funds wait"));
    assert!(page(detail.clone()).await.contains("<td>Sent</td>"));

    // The sweep stays open for 40 minutes.
    let tx1 = sweeps(&t, id).await[0].1.clone();
    set_state(&t, &tx1, SweepState::Pending);
    sqlx::query("UPDATE sweeps SET created_at = now() - interval '40 minutes' WHERE payment_id = $1")
        .bind(id)
        .execute(&t.app.store.pool)
        .await
        .unwrap();
    let html = page(overview.clone()).await;
    assert!(html.contains("Funds wait at 1 deposit address"), "{html}");
    assert!(html.contains(&format!("Payment {id}</a>: Sweep sent 40 min ago, not final yet.")), "{html}");
    assert!(page(detail.clone()).await.contains("Sweep sent 40 min ago, not final yet."));

    // It failed, and the new tries fail too: the funds are confirmed but not swept.
    set_state(&t, &tx1, SweepState::Failed("out of energy".into()));
    turmite::worker::process_payment(&t.app, id).await.unwrap();
    sqlx::query("UPDATE sweeps SET status = 'failed', last_error = 'out of energy' WHERE payment_id = $1")
        .bind(id)
        .execute(&t.app.store.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE payments SET swept_gross = 0 WHERE payment_id = $1").bind(id).execute(&t.app.store.pool).await.unwrap();
    sqlx::query("UPDATE payment_transactions SET confirmed_at = now() - interval '2 hours' WHERE payment_id = $1")
        .bind(id)
        .execute(&t.app.store.pool)
        .await
        .unwrap();
    let html = page(overview).await;
    assert!(html.contains("Sweep failed: out of energy. The worker tries again."), "{html}");
    let html = page(detail).await;
    assert!(html.contains("Failed: out of energy"), "{html}");
}
