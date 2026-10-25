//! Merchant API and payment lifecycle on mock chains.

mod common;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use axum::http::{HeaderMap, StatusCode};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::{Sha256, Sha512};
use turmite::chains::{DriverRegistry, driver::MockDriver};

type Captured = Arc<Mutex<Vec<(HeaderMap, String)>>>;

/// A receiver that answers 500 to the first `fail_first` requests, then 200.
async fn receiver(fail_first: usize) -> (String, Captured) {
    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let c2 = captured.clone();
    let count = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().route(
        "/hook",
        axum::routing::post(move |headers: HeaderMap, body: String| {
            let c = c2.clone();
            let n = count.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < fail_first {
                    return StatusCode::INTERNAL_SERVER_ERROR;
                }
                c.lock().unwrap().push((headers, body));
                StatusCode::OK
            }
        }),
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/hook"), captured)
}

async fn deliver(t: &common::TestApp) {
    turmite::webhooks::deliver_due(&t.app).await.unwrap();
}

async fn create(t: &common::TestApp, key: &str, extra: Value) -> Value {
    let mut body = json!({ "price_amount": 100, "price_currency": "usd", "pay_currency": "btc" });
    body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    let (status, p) = t.post("/v1/payment", key, &body).await;
    assert_eq!(status, 201, "{p}");
    p
}

fn amount(p: &Value, fraction: f64) -> String {
    format!("{:.8}", p["pay_amount"].as_f64().unwrap() * fraction)
}

async fn payment(t: &common::TestApp, key: &str, id: i64) -> Value {
    t.get(&format!("/v1/payment/{id}"), key).await.1
}

#[tokio::test]
async fn ipn_is_signed_twice_and_verifiable() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let (url, got) = receiver(0).await;
    let p = create(&t, &key, json!({ "ipn_callback_url": url })).await;
    let id = common::pid(&p);
    t.deposit(id, &amount(&p, 1.0), 1).await;
    deliver(&t).await;

    let got = got.lock().unwrap().clone();
    let (headers, body) = got.iter().find(|(_, b)| b.contains("\"finished\"")).expect("finished IPN").clone();
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["payment_id"], id);
    // The body is the key-sorted text: parsing and writing it again gives the same bytes.
    assert_eq!(turmite::webhooks::sorted_json(&v), body);

    // Compatible signature: HMAC-SHA512 over the key-sorted JSON, in the project's header.
    let mut mac = <Hmac<Sha512>>::new_from_slice(b"test-ipn-secret").unwrap();
    mac.update(turmite::webhooks::sorted_json(&v).as_bytes());
    assert_eq!(headers["x-turmite-sig"].to_str().unwrap(), hex::encode(mac.finalize().into_bytes()));

    // v1 signature over "<timestamp>.<raw body>".
    let ts = headers["x-turmite-timestamp"].to_str().unwrap();
    let mut mac = <Hmac<Sha256>>::new_from_slice(b"test-ipn-secret").unwrap();
    mac.update(format!("{ts}.{body}").as_bytes());
    assert_eq!(headers["x-turmite-signature"].to_str().unwrap(), format!("v1={}", hex::encode(mac.finalize().into_bytes())));
    assert_eq!(headers["x-turmite-event"].to_str().unwrap(), "payment.finished");
    assert!(headers.contains_key("x-turmite-delivery"));
}

#[tokio::test]
async fn custom_signature_header_name() {
    let t = common::start(Default::default()).await;
    let (pid, key) = t.project().await;
    t.app.store.set_project_payment_settings(pid, "x-legacy-sig", 60, 1440).await.unwrap();
    let (url, got) = receiver(0).await;
    let p = create(&t, &key, json!({ "ipn_callback_url": url })).await;
    t.deposit(common::pid(&p), &amount(&p, 1.0), 1).await;
    deliver(&t).await;
    let got = got.lock().unwrap().clone();
    assert!(!got.is_empty());
    assert!(got.iter().all(|(h, _)| h.contains_key("x-legacy-sig") && !h.contains_key("x-turmite-sig")));
}

#[tokio::test]
async fn partial_payment_stays_watched_and_top_up_finishes() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let p = create(&t, &key, json!({})).await;
    let id = common::pid(&p);

    t.deposit(id, &amount(&p, 0.5), 1).await;
    let v = payment(&t, &key, id).await;
    assert_eq!(v["payment_status"], "partially_paid");
    assert!(v["outcome_amount"].as_f64().unwrap() > 0.0, "partial funds are credited");
    assert!(t.app.store.active_payment_ids().await.unwrap().contains(&id), "partly paid stays watched");

    t.deposit(id, &amount(&p, 0.5), 1).await;
    assert_eq!(payment(&t, &key, id).await["payment_status"], "finished");
}

#[tokio::test]
async fn top_up_and_overpayment_are_credited_and_notified() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let (url, got) = receiver(0).await;
    let p = create(&t, &key, json!({ "ipn_callback_url": url })).await;
    let id = common::pid(&p);
    let pay = p["pay_amount"].as_f64().unwrap();

    t.deposit(id, &amount(&p, 0.9), 1).await;
    t.deposit(id, &amount(&p, 0.2), 1).await;
    let v = payment(&t, &key, id).await;
    assert_eq!(v["payment_status"], "finished");
    let after_two = v["outcome_amount"].as_f64().unwrap();
    assert!((after_two - pay * 1.1).abs() < pay * 0.005);

    t.deposit(id, &amount(&p, 0.3), 1).await;
    let v = payment(&t, &key, id).await;
    assert_eq!(v["payment_status"], "finished");
    let after_three = v["outcome_amount"].as_f64().unwrap();
    assert!((after_three - pay * 1.4).abs() < pay * 0.005, "every deposit is credited: {after_three}");

    let (_, bal) = t.get("/v1/balance", &key).await;
    assert!((bal["btc"]["amount"].as_f64().unwrap() - after_three).abs() < 1e-9, "{bal}");

    deliver(&t).await;
    let credited: Vec<f64> = got
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(_, b)| serde_json::from_str::<Value>(b).ok())
        .filter_map(|v| v["outcome_amount"].as_f64())
        .collect();
    assert!(credited.iter().any(|&o| (o - pay * 1.4).abs() < pay * 0.005), "the last credit is notified: {credited:?}");
}

#[tokio::test]
async fn late_payment_is_credited_as_paid_late() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let (url, got) = receiver(0).await;
    let p = create(&t, &key, json!({ "ipn_callback_url": url })).await;
    let id = common::pid(&p);
    sqlx::query("UPDATE payments SET expires_at = now() - interval '1 hour' WHERE payment_id = $1")
        .bind(id)
        .execute(&t.app.store.pool)
        .await
        .unwrap();
    t.tick().await;
    assert_eq!(payment(&t, &key, id).await["payment_status"], "expired");

    t.deposit(id, &amount(&p, 1.0), 1).await;
    let settlement: String =
        sqlx::query_scalar("SELECT settlement FROM payments WHERE payment_id = $1").bind(id).fetch_one(&t.app.store.pool).await.unwrap();
    assert_eq!(settlement, "paid_late");
    let v = payment(&t, &key, id).await;
    assert_eq!(v["payment_status"], "finished", "{v}");
    deliver(&t).await;
    assert!(got.lock().unwrap().iter().any(|(_, b)| b.contains("\"finished\"")));
}

#[tokio::test]
async fn one_percent_tolerance_boundary() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let low = create(&t, &key, json!({})).await;
    t.deposit(common::pid(&low), &amount(&low, 0.985), 1).await;
    assert_eq!(payment(&t, &key, common::pid(&low)).await["payment_status"], "partially_paid");

    let high = create(&t, &key, json!({})).await;
    t.deposit(common::pid(&high), &amount(&high, 0.992), 1).await;
    assert_eq!(payment(&t, &key, common::pid(&high)).await["payment_status"], "finished");
}

#[tokio::test]
async fn idempotency_key_returns_the_same_payment() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let a = create(&t, &key, json!({ "idempotency_key": "order-1" })).await;
    let b = create(&t, &key, json!({ "idempotency_key": "order-1" })).await;
    assert_eq!(a["payment_id"], b["payment_id"]);
    assert_eq!(a["pay_address"], b["pay_address"]);

    let r = t
        .client
        .post(t.url("/v1/payment"))
        .header("x-api-key", &key)
        .header("idempotency-key", "order-2")
        .json(&json!({ "price_amount": 5, "price_currency": "usd", "pay_currency": "eth" }))
        .send()
        .await
        .unwrap();
    let c: Value = r.json().await.unwrap();
    let r = t
        .client
        .post(t.url("/v1/payment"))
        .header("x-api-key", &key)
        .header("idempotency-key", "order-2")
        .json(&json!({ "price_amount": 5, "price_currency": "usd", "pay_currency": "eth" }))
        .send()
        .await
        .unwrap();
    let d: Value = r.json().await.unwrap();
    assert_eq!(c["payment_id"], d["payment_id"]);
}

#[tokio::test]
async fn failing_receiver_is_retried_until_delivered() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let (url, got) = receiver(2).await;
    let p = create(&t, &key, json!({ "ipn_callback_url": url })).await;
    t.deposit(common::pid(&p), &amount(&p, 1.0), 1).await;
    for _ in 0..4 {
        deliver(&t).await;
        sqlx::query("UPDATE webhook_deliveries SET next_retry_at = now() WHERE status = 'failed'")
            .execute(&t.app.store.pool)
            .await
            .unwrap();
    }
    assert!(got.lock().unwrap().iter().any(|(_, b)| b.contains("\"finished\"")), "delivered after retries");
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM webhook_deliveries WHERE status <> 'delivered'")
        .fetch_one(&t.app.store.pool)
        .await
        .unwrap();
    assert_eq!(pending, 0);
}

#[tokio::test]
async fn final_events_go_dead_after_the_schedule() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let (url, _) = receiver(usize::MAX).await;
    let p = create(&t, &key, json!({ "ipn_callback_url": url })).await;
    t.deposit(common::pid(&p), &amount(&p, 1.0), 1).await;
    for _ in 0..12 {
        deliver(&t).await;
        sqlx::query("UPDATE webhook_deliveries SET next_retry_at = now() WHERE status = 'failed'")
            .execute(&t.app.store.pool)
            .await
            .unwrap();
    }
    let (status, attempts): (String, i32) = sqlx::query_as(
        "SELECT status, attempt_count FROM webhook_deliveries WHERE event_type = 'payment.finished' AND webhook_endpoint_id IS NULL",
    )
    .fetch_one(&t.app.store.pool)
    .await
    .unwrap();
    assert_eq!((status.as_str(), attempts), ("dead", 11));
}

#[tokio::test]
async fn projects_are_isolated() {
    let t = common::start(Default::default()).await;
    let (_, key_a) = t.project().await;
    let (_, key_b) = t.project().await;
    let p = create(&t, &key_a, json!({})).await;
    let (status, _) = t.get(&format!("/v1/payment/{}", common::pid(&p)), &key_b).await;
    assert_eq!(status, 404);
    let (_, list) = t.get("/v1/payment?limit=50", &key_b).await;
    assert_eq!(list["total"], 0);
}

#[tokio::test]
async fn listing_filters_and_pages() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    for i in 0..5 {
        create(&t, &key, json!({ "order_id": format!("o-{i}") })).await;
    }
    let (_, page) = t.get("/v1/payment?limit=2&page=1&orderBy=asc", &key).await;
    assert_eq!(page["total"], 5);
    assert_eq!(page["pagesCount"], 3);
    assert_eq!(page["data"].as_array().unwrap().len(), 2);
    assert_eq!(page["data"][0]["order_id"], "o-2");
    let (_, one) = t.get("/v1/payment?order_id=o-4", &key).await;
    assert_eq!(one["total"], 1);
}

#[tokio::test]
async fn currencies_estimate_and_numbers() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let (_, c) = t.get("/v1/currencies", &key).await;
    let list: Vec<&str> = c["currencies"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    for ticker in ["btc", "xmr", "eth", "usdc", "ethbase", "trx", "usdttrc20", "sol", "ton"] {
        assert!(list.contains(&ticker), "{ticker} missing from {list:?}");
    }
    let (_, coins) = t.get("/v1/merchant/coins", &key).await;
    assert!(coins["selectedCurrencies"].as_array().unwrap().len() >= 9);

    let (status, e) = t.get("/v1/estimate?amount=100&currency_from=usd&currency_to=btc", &key).await;
    assert_eq!(status, 200, "{e}");
    assert_eq!(e["estimated_amount"], "0.001");
    assert_eq!(e["amount_from"], json!(100));

    let (_, min) = t.get("/v1/min-amount?currency_from=btc&currency_to=btc&fiat_equivalent=usd", &key).await;
    assert!(min["min_amount"].is_number());

    let (_, full) = t.get("/v1/full-currencies", &key).await;
    let btc = full["currencies"].as_array().unwrap().iter().find(|c| c["ticker"] == "btc").unwrap().clone();
    assert_eq!(btc["enable"], true);
    assert_eq!(btc["network"], "btc");
}

#[tokio::test]
async fn coins_without_a_connected_node_are_not_offered() {
    let mut drivers = turmite::chains::driver::DriverRegistry::new();
    drivers.insert(Arc::new(turmite::chains::driver::MockDriver::new("bitcoin")));
    let t = common::start(common::TestOptions { drivers: Some(drivers), ..Default::default() }).await;
    let (_, key) = t.project().await;
    assert_eq!(t.get("/v1/currencies", &key).await.1["currencies"], json!(["btc"]));
    assert_eq!(t.get("/v1/merchant/coins", &key).await.1["selectedCurrencies"], json!(["btc"]));
    let body = json!({ "price_amount": 10, "price_currency": "usd", "pay_currency": "eth" });
    let (status, e) = t.post("/v1/payment", &key, &body).await;
    assert_eq!(status, 503, "{e}");

    let (_, inv) = t.post("/v1/invoice", &key, &json!({ "price_amount": 10, "price_currency": "usd" })).await;
    let html = t.client.get(inv["invoice_url"].as_str().unwrap()).send().await.unwrap().text().await.unwrap();
    assert!(html.contains("value=\"btc\"") && !html.contains("value=\"eth\""));
}

#[tokio::test]
async fn stablecoins_are_priced_at_one_dollar() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let (status, p) =
        t.post("/v1/payment", &key, &json!({ "price_amount": 25.5, "price_currency": "usd", "pay_currency": "usdttrc20" })).await;
    assert_eq!(status, 201, "{p}");
    assert_eq!(p["pay_amount"], json!(25.5));
    assert_eq!(p["network"], "tron");
}

#[tokio::test]
async fn invoice_flow_without_javascript() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let (status, inv) = t
        .post(
            "/v1/invoice",
            &key,
            &json!({ "price_amount": 49, "price_currency": "usd", "order_id": "inv-1", "success_url": "https://shop.example.com/ok" }),
        )
        .await;
    assert_eq!(status, 201, "{inv}");
    let url = inv["invoice_url"].as_str().unwrap().to_string();
    let page = t.client.get(&url).send().await.unwrap();
    assert_eq!(page.status(), 200);
    let csp = page.headers()["content-security-policy"].to_str().unwrap().to_string();
    assert!(csp.contains("default-src 'none'") && !csp.contains("script-src"), "{csp}");
    let html = page.text().await.unwrap();
    assert!(!html.to_lowercase().contains("<script"), "customer pages carry no script");
    assert!(html.contains("value=\"btc\""));

    let r = t.client.post(&url).header("origin", &t.base).form(&[("currency", "btc")]).send().await.unwrap();
    assert_eq!(r.status(), 303);
    let to = r.headers()["location"].to_str().unwrap().to_string();
    assert!(to.starts_with("/pay/"), "{to}");
    let pay_html = t.client.get(t.url(&to)).send().await.unwrap().text().await.unwrap();
    assert!(!pay_html.to_lowercase().contains("<script"));
    assert!(pay_html.contains("<svg"), "inline QR");
    let status_html = t.client.get(t.url(&format!("{to}/status"))).send().await.unwrap().text().await.unwrap();
    assert!(status_html.contains("http-equiv=\"refresh\""));

    // The same coin again reuses the open payment; the API can also pay an invoice.
    let again = t.client.post(&url).form(&[("currency", "btc")]).send().await.unwrap();
    assert_eq!(again.headers()["location"].to_str().unwrap(), to);
    let (status, p) = t.post("/v1/invoice-payment", &key, &json!({ "iid": inv["id"], "pay_currency": "eth" })).await;
    assert_eq!(status, 201, "{p}");
    assert_eq!(p["invoice_id"].as_i64().unwrap().to_string(), inv["id"].as_str().unwrap());
}

#[tokio::test]
async fn bad_input_is_rejected_clearly() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    for body in [
        json!({ "price_amount": 0, "price_currency": "usd", "pay_currency": "btc" }),
        json!({ "price_amount": 1, "price_currency": "usd", "pay_currency": "nope" }),
        json!({ "price_amount": 1, "price_currency": "usd", "pay_currency": "btc", "ipn_callback_url": "ftp://x" }),
        json!({ "price_amount": 1, "price_currency": "usd", "pay_currency": "btc", "success_url": "https://u:p@x.com" }),
    ] {
        let (status, e) = t.post("/v1/payment", &key, &body).await;
        assert_eq!(status, 400, "{body} -> {e}");
        assert_eq!(e["status"], false);
        assert_eq!(e["statusCode"], 400);
        assert!(e["message"].as_str().unwrap().len() > 5);
    }
    let (status, e) = t.get("/v1/payment", "").await;
    assert_eq!((status, e["code"].as_str()), (401, Some("AUTH_REQUIRED")));
}

#[tokio::test]
async fn refunded_payments_leave_the_balance() {
    let t = common::start(Default::default()).await;
    let (pid, key) = t.project().await;
    let kept = create(&t, &key, json!({})).await;
    let refunded = create(&t, &key, json!({})).await;
    t.deposit(common::pid(&kept), &amount(&kept, 1.0), 1).await;
    t.deposit(common::pid(&refunded), &amount(&refunded, 1.0), 1).await;
    assert!(t.app.store.mark_refunded(pid, common::pid(&refunded)).await.unwrap());
    let (_, bal) = t.get("/v1/balance", &key).await;
    assert!((bal["btc"]["amount"].as_f64().unwrap() - kept["pay_amount"].as_f64().unwrap()).abs() < 1e-12, "{bal}");
}

#[tokio::test]
async fn payments_below_the_minimum_are_refused() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let (_, min) = t.get("/v1/min-amount?currency_from=btc", &key).await;
    assert_eq!(min["min_amount"].to_string(), "1e-6", "the default minimum for 8 decimals");

    let below = [
        json!({ "price_amount": 1, "price_currency": "usd", "pay_currency": "btc", "pay_amount": "0.0000005" }),
        // 0.01 USD at 100 000 USD per BTC is 0.0000001 BTC.
        json!({ "price_amount": 0.01, "price_currency": "usd", "pay_currency": "btc" }),
    ];
    for body in below {
        let (status, e) = t.post("/v1/payment", &key, &body).await;
        assert_eq!((status, e["code"].as_str()), (400, Some("INVALID_REQUEST_PARAMS")), "{e}");
        assert!(e["message"].as_str().unwrap().contains("0.000001 btc"), "{e}");
    }
    let (status, p) = t
        .post("/v1/payment", &key, &json!({ "price_amount": 1, "price_currency": "usd", "pay_currency": "btc", "pay_amount": "0.000001" }))
        .await;
    assert_eq!(status, 201, "{p}");
}

#[tokio::test]
async fn a_return_to_an_earlier_status_is_notified_again() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let p = create(&t, &key, json!({ "ipn_callback_url": "http://127.0.0.1:9/ipn" })).await;
    let id = common::pid(&p);
    t.deposit(id, &amount(&p, 1.0), 0).await;
    assert_eq!(payment(&t, &key, id).await["payment_status"], "confirming");
    // The deposit leaves the chain: the payment waits again.
    sqlx::query("UPDATE payment_transactions SET dropped_at = now() WHERE payment_id = $1")
        .bind(id)
        .execute(&t.app.store.pool)
        .await
        .unwrap();
    turmite::worker::recompute(&t.app, id).await.unwrap();
    assert_eq!(payment(&t, &key, id).await["payment_status"], "waiting");
    t.deposit(id, &amount(&p, 1.0), 0).await;
    assert_eq!(payment(&t, &key, id).await["payment_status"], "confirming");
    turmite::worker::recompute(&t.app, id).await.unwrap();

    let events: Vec<String> =
        sqlx::query_scalar("SELECT event_type FROM webhook_deliveries WHERE webhook_endpoint_id IS NULL ORDER BY webhook_delivery_id")
            .fetch_all(&t.app.store.pool)
            .await
            .unwrap();
    assert_eq!(events, ["payment.waiting", "payment.confirming", "payment.waiting", "payment.confirming"]);
}

#[tokio::test]
async fn a_later_deposit_on_a_finished_payment_changes_actually_paid() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let (url, got) = receiver(0).await;
    let p = create(&t, &key, json!({ "ipn_callback_url": url })).await;
    let id = common::pid(&p);
    let pay = p["pay_amount"].as_f64().unwrap();
    t.deposit(id, &amount(&p, 1.0), 1).await;
    assert_eq!(payment(&t, &key, id).await["payment_status"], "finished");

    // A later deposit, not confirmed yet: the amount received changes and the shop gets an update.
    t.deposit(id, &amount(&p, 0.5), 0).await;
    let v = payment(&t, &key, id).await;
    assert_eq!(v["payment_status"], "finished");
    assert!((v["actually_paid"].as_f64().unwrap() - pay * 1.5).abs() < 1e-12, "{v}");
    deliver(&t).await;
    let finished: Vec<f64> = got
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(_, b)| serde_json::from_str::<Value>(b).ok())
        .filter(|v| v["payment_status"] == "finished")
        .filter_map(|v| v["actually_paid"].as_f64())
        .collect();
    assert_eq!(finished.len(), 2, "{finished:?}");
    assert!((finished[1] - pay * 1.5).abs() < 1e-12, "{finished:?}");
}

#[tokio::test]
async fn a_failed_create_gives_the_deposit_address_back() {
    // The Tron driver of this process cannot derive: Tron addresses come from the pool.
    let mut drivers = DriverRegistry::new();
    for chain in ["bitcoin", "ethereum"] {
        drivers.insert(Arc::new(MockDriver::new(chain)));
    }
    drivers.insert(Arc::new(MockDriver::without_derive("tron")));
    let t = common::start(common::TestOptions { drivers: Some(drivers), ..Default::default() }).await;
    let (_, key) = t.project().await;
    let store = &t.app.store;
    let tron = store.list_networks().await.unwrap().into_iter().find(|n| n.chain == "tron").unwrap();
    let pooled = store.insert_deposit_address(tron.network_id, None, 7, "TBMspfKq6FS8hXjXbxmFrpiN8xXmreBSmP", None).await.unwrap();
    sqlx::query("CREATE FUNCTION no_payments() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'test'; END $$")
        .execute(&store.pool)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER no_payments BEFORE INSERT ON payments FOR EACH ROW EXECUTE FUNCTION no_payments()")
        .execute(&store.pool)
        .await
        .unwrap();

    let trx = json!({ "price_amount": 3, "price_currency": "usd", "pay_currency": "trx" });
    assert_eq!(t.post("/v1/payment", &key, &trx).await.0, 500);
    assert_eq!(t.post("/v1/payment", &key, &json!({ "price_amount": 3, "price_currency": "usd", "pay_currency": "btc" })).await.0, 500);
    // The pool address is free again and the derived Bitcoin address is deleted.
    let owner: Option<i64> = sqlx::query_scalar("SELECT project_id FROM deposit_addresses WHERE deposit_address_id = $1")
        .bind(pooled)
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(owner, None);
    let reserved: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM deposit_addresses WHERE payment_id IS NULL AND project_id IS NOT NULL")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(reserved, 0);

    sqlx::query("DROP TRIGGER no_payments ON payments").execute(&store.pool).await.unwrap();
    let (status, p) = t.post("/v1/payment", &key, &trx).await;
    assert_eq!(status, 201, "{p}");
    assert_eq!(p["pay_address"], "TBMspfKq6FS8hXjXbxmFrpiN8xXmreBSmP");
}
