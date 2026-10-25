//! The public API contract: field types, list filters and error bodies that typed clients of the
//! common hosted-gateway API depend on.

mod common;

use serde_json::{Value, json};

async fn create(t: &common::TestApp, key: &str, extra: Value) -> Value {
    let mut body = json!({ "price_amount": 100, "price_currency": "usd", "pay_currency": "btc" });
    body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    let (status, p) = t.post("/v1/payment", key, &body).await;
    assert_eq!(status, 201, "{p}");
    p
}

async fn payloads(t: &common::TestApp, from_endpoint: bool) -> Vec<Value> {
    sqlx::query_scalar("SELECT payload FROM webhook_deliveries WHERE (webhook_endpoint_id IS NOT NULL) = $1 ORDER BY webhook_delivery_id")
        .bind(from_endpoint)
        .fetch_all(&t.app.store.pool)
        .await
        .unwrap()
}

fn assert_error(status: u16, e: &Value, want_status: u16, want_code: &str) {
    assert_eq!(status, want_status, "{e}");
    assert_eq!(e["status"], false, "{e}");
    assert_eq!(e["statusCode"], want_status, "{e}");
    assert_eq!(e["code"], want_code, "{e}");
    assert!(e["message"].as_str().is_some_and(|m| !m.is_empty()), "{e}");
}

#[tokio::test]
async fn payment_ids_have_the_type_of_each_place() {
    let t = common::start(Default::default()).await;
    let (pid, key) = t.project().await;
    let secret = t.app.sealer.seal_str("webhook_secret", "endpoint-secret");
    t.app.store.add_webhook_endpoint(pid, "http://127.0.0.1:9/endpoint", &secret).await.unwrap();

    // Create: both ids are strings.
    let p = create(&t, &key, json!({ "ipn_callback_url": "http://127.0.0.1:9/ipn" })).await;
    assert!(p["payment_id"].is_string(), "{p}");
    assert!(p["purchase_id"].as_str().is_some_and(|s| s.parse::<u64>().is_ok()), "{p}");
    let id = common::pid(&p);

    // Status: both ids are numbers, and every documented field is present.
    let (status, g) = t.get(&format!("/v1/payment/{id}"), &key).await;
    assert_eq!(status, 200, "{g}");
    assert_eq!(g["payment_id"], json!(id));
    assert_eq!(g["purchase_id"].as_u64().map(|n| n.to_string()), p["purchase_id"].as_str().map(str::to_string));
    for field in [
        "payment_id",
        "invoice_id",
        "payment_status",
        "pay_address",
        "payin_extra_id",
        "price_amount",
        "price_currency",
        "pay_amount",
        "actually_paid",
        "pay_currency",
        "order_id",
        "order_description",
        "purchase_id",
        "outcome_amount",
        "outcome_currency",
        "network",
        "network_precision",
        "smart_contract",
        "payin_hash",
        "payout_hash",
        "payment_url",
        "created_at",
        "updated_at",
        "expiration_estimate_date",
        "parent_payment_id",
        "type",
        "time_limit",
        "burning_percent",
        "payment_extra_ids",
        "amount_received",
        "actually_paid_at_fiat",
        "fee",
    ] {
        assert!(g.get(field).is_some(), "{field} missing from {g}");
    }
    assert!(g["network_precision"].is_i64());
    assert_eq!(g["price_amount"], json!(100), "whole amounts are integers");
    for (field, want) in [
        ("parent_payment_id", Value::Null),
        ("time_limit", Value::Null),
        ("burning_percent", Value::Null),
        ("payment_extra_ids", Value::Null),
    ] {
        assert_eq!(g[field], want, "{field}");
    }
    assert_eq!(g["type"], "crypto2crypto");
    assert_eq!(g["amount_received"], g["pay_amount"], "before a credit: the amount due");
    assert_eq!(g["actually_paid_at_fiat"], json!(0));
    assert_eq!(g["fee"], json!({ "currency": "btc", "depositFee": 0, "withdrawalFee": 0, "serviceFee": 0 }));

    // A purchase_id that is not a plain number stays a string.
    let other = create(&t, &key, json!({ "purchase_id": "order-7" })).await;
    let (_, g) = t.get(&format!("/v1/payment/{}", common::pid(&other)), &key).await;
    assert_eq!(g["purchase_id"], "order-7");

    // List: the status shape.
    let (_, list) = t.get("/v1/payment", &key).await;
    assert_eq!(list["limit"], 10);
    assert!(list["data"].as_array().unwrap().iter().all(|d| d["payment_id"].is_i64()), "{list}");
    assert!(list["data"].as_array().unwrap().iter().all(|d| d["type"] == "crypto2crypto" && d["fee"].is_object()), "{list}");

    // Webhooks: payment_id is a number, purchase_id a string. The envelope time is Unix seconds.
    let ipn = payloads(&t, false).await;
    assert_eq!(ipn[0]["payment_id"], json!(id));
    assert_eq!(ipn[0]["purchase_id"], p["purchase_id"]);
    assert_eq!(ipn[0]["type"], "crypto2crypto");
    assert_eq!(ipn[0]["fee"]["currency"], "btc");
    assert!(p["fee"].is_object() && p["parent_payment_id"].is_null(), "create has the fields too: {p}");

    // After a full deposit: the credited amount and the fiat value at the payment's rate (100 000 USD per BTC).
    t.deposit(id, "0.001", 1).await;
    let (_, g) = t.get(&format!("/v1/payment/{id}"), &key).await;
    assert_eq!(g["payment_status"], "finished", "{g}");
    assert_eq!(g["amount_received"], g["outcome_amount"]);
    assert_eq!(g["actually_paid_at_fiat"], json!(100));
    let events = payloads(&t, true).await;
    let event = events.iter().find(|e| e["data"]["payment_id"] == json!(id)).expect("endpoint event");
    assert_eq!(event["event"], "payment.waiting");
    assert!(event["delivery_id"].is_string());
    assert!(event["created_at"].as_i64().is_some_and(|s| s > 1_700_000_000 && s < 10_000_000_000), "{event}");
    assert_eq!(event["data"]["purchase_id"], p["purchase_id"]);
}

#[tokio::test]
async fn invoices_and_payment_lists() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let (status, inv) = t.post("/v1/invoice", &key, &json!({ "price_amount": 49.5, "price_currency": "USD", "order_id": "inv-1" })).await;
    assert_eq!(status, 201, "{inv}");
    assert!(inv["id"].as_str().is_some_and(|s| s.parse::<i64>().is_ok()), "{inv}");
    assert_eq!(inv["price_amount"], "49.5");
    assert_eq!(inv["price_currency"], "usd");
    assert_eq!(inv["updated_at"], inv["created_at"]);
    for field in ["token_id", "pay_currency", "ipn_callback_url", "invoice_url", "success_url", "cancel_url", "expiration_estimate_date"] {
        assert!(inv.get(field).is_some(), "{field} missing from {inv}");
    }

    let (status, ip) = t.post("/v1/invoice-payment", &key, &json!({ "iid": inv["id"], "pay_currency": "eth" })).await;
    assert_eq!(status, 201, "{ip}");
    assert!(ip["payment_id"].is_string(), "{ip}");
    create(&t, &key, json!({ "price_amount": 5 })).await;
    create(&t, &key, json!({ "price_amount": 250 })).await;

    let (_, by_invoice) = t.get(&format!("/v1/payment?invoiceId={}", inv["id"].as_str().unwrap()), &key).await;
    assert_eq!(by_invoice["total"], 1, "{by_invoice}");
    assert_eq!(by_invoice["data"][0]["payment_id"].to_string(), ip["payment_id"].as_str().unwrap());
    let (status, e) = t.get("/v1/payment?invoiceId=abc", &key).await;
    assert_error(status, &e, 400, "INVALID_REQUEST_PARAMS");

    let (_, sorted) = t.get("/v1/payment?sortBy=price_amount&orderBy=asc", &key).await;
    let prices: Vec<f64> = sorted["data"].as_array().unwrap().iter().map(|d| d["price_amount"].as_f64().unwrap()).collect();
    assert_eq!(prices, vec![5.0, 49.5, 250.0]);
    let (_, by_coin) = t.get("/v1/payment?sortBy=pay_currency&orderBy=desc", &key).await;
    assert_eq!(by_coin["data"][0]["pay_currency"], "eth", "{by_coin}");
}

#[tokio::test]
async fn reference_endpoints_match_the_documented_shapes() {
    let t = common::start(Default::default()).await;
    let (pid, key) = t.project().await;

    let (_, full) = t.get("/v1/full-currencies", &key).await;
    let usdt = full["currencies"].as_array().unwrap().iter().find(|c| c["ticker"] == "usdttrc20").unwrap().clone();
    assert_eq!(usdt["code"], "USDTTRC20");
    assert_eq!(usdt["network"], "tron");
    assert_eq!(usdt["network_precision"], "6");
    assert_eq!(usdt["enable"], true);
    assert!(usdt["smart_contract"].is_string());
    assert!(usdt.get("precision").is_none());

    let (status, min) = t.get("/v1/min-amount?currency_from=usdttrc20", &key).await;
    assert_eq!(status, 200, "{min}");
    assert_eq!((min["currency_from"].as_str(), min["currency_to"].as_str()), (Some("usdttrc20"), Some("usdttrc20")));
    assert!(min["min_amount"].is_number());
    assert!(min.get("fiat_equivalent").is_none(), "{min}");
    let (_, min) = t.get("/v1/min-amount?currency_from=btc&currency_to=usdttrc20&fiat_equivalent=usd", &key).await;
    assert_eq!(min["currency_to"], "usdttrc20");
    assert!(min["fiat_equivalent"].is_number(), "{min}");

    let (_, est) = t.get("/v1/estimate?amount=100&currency_from=usd&currency_to=btc", &key).await;
    assert!(est["estimated_amount"].is_string(), "{est}");

    let (status, ok) =
        t.post("/v1/payout/validate-address", &key, &json!({ "address": format!("0x{}", "ab".repeat(20)), "currency": "eth" })).await;
    assert_eq!((status, ok), (200, json!("OK")));
    let (status, e) = t.post("/v1/payout/validate-address", &key, &json!({ "address": "0x12", "currency": "eth" })).await;
    assert_error(status, &e, 400, "INVALID_REQUEST_PARAMS");
    // The same checks as the wallet forms: EIP-55, checksums and the network of the address.
    for (currency, address, valid) in [
        ("eth", "0x70997970C51812dc3A010C7d01b50e0d17dc79C8", true),
        ("eth", "0x70997970C51812dc3A010C7d01b50e0d17dc79c8", false),
        ("btc", "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx", true),
        ("btc", "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsy", false),
        ("btc", "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4", false),
        ("xmr", "48x8jjw6FYQHmGRW2mKTnR83bKy7oRez9NWwVQiqkqxf4uuu9X9kKNbY1bdHKPS5Zo4968N8Ts2fMeDSPvYvkHnM5vjiiVL", true),
        ("xmr", "48x8jjw6FYQHmGRW2mKTnR83bKy7oRez9NWwVQiqkqxf4uuu9X9kKNbY1bdHKPS5Zo4968N8Ts2fMeDSPvYvkHnM5vjii", false),
        ("trx", "TBMspfKq6FS8hXjXbxmFrpiN8xXmreBSmQ", false),
    ] {
        let (status, _) = t.post("/v1/payout/validate-address", &key, &json!({ "address": address, "currency": currency })).await;
        assert_eq!(status, if valid { 200 } else { 400 }, "{currency} {address}");
    }

    // Payout list: default page size and shape.
    let hash = turmite::auth::hash_password("a long enough password").await.unwrap();
    let uid = t.app.store.create_user("ops@shop.test", &hash, "member").await.unwrap();
    t.app.store.set_member(pid, uid, "project_admin").await.unwrap();
    let r = t
        .client
        .post(t.url("/v1/auth"))
        .json(&json!({ "email": "ops@shop.test", "password": "a long enough password" }))
        .send()
        .await
        .unwrap();
    let token = r.json::<Value>().await.unwrap()["token"].as_str().unwrap().to_string();
    let r = t.client.get(t.url("/v1/payout")).header("x-api-key", &key).bearer_auth(&token).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let list: Value = r.json().await.unwrap();
    assert_eq!(list, json!({ "payouts": [], "limit": 10, "page": 0, "pagesCount": 0, "total": 0 }));
}

#[tokio::test]
async fn every_api_error_is_json() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;

    let (status, e) = t.post("/v1/payment", &key, &json!({ "price_amount": 1, "pay_currency": "btc" })).await;
    assert_error(status, &e, 400, "INVALID_REQUEST_PARAMS");
    assert!(e["message"].as_str().unwrap().contains("price_currency"), "{e}");

    let r = t
        .client
        .post(t.url("/v1/payment"))
        .header("x-api-key", &key)
        .header("content-type", "application/json")
        .body("{")
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    assert_error(status, &r.json().await.unwrap(), 400, "INVALID_REQUEST_PARAMS");

    let r = t.client.post(t.url("/v1/invoice")).header("x-api-key", &key).body(r#"{"price_amount":1}"#).send().await.unwrap();
    let status = r.status().as_u16();
    assert_error(status, &r.json().await.unwrap(), 400, "INVALID_REQUEST_PARAMS");

    let (status, e) = t.get("/v1/min-amount", &key).await;
    assert_error(status, &e, 400, "INVALID_REQUEST_PARAMS");
    let (status, e) = t.get("/v1/payment?limit=abc", &key).await;
    assert_error(status, &e, 400, "INVALID_REQUEST_PARAMS");
    let (status, e) = t.get("/v1/payment/123", &key).await;
    assert_error(status, &e, 404, "NOT_FOUND");
    let (status, e) = t.get("/v1/no-such-endpoint", &key).await;
    assert_error(status, &e, 404, "NOT_FOUND");

    let r = t.client.delete(t.url("/v1/payment")).header("x-api-key", &key).send().await.unwrap();
    let status = r.status().as_u16();
    assert_error(status, &r.json().await.unwrap(), 405, "METHOD_NOT_ALLOWED");

    let r = t.client.post(t.url("/v1/payout/fee")).header("x-api-key", &key).send().await.unwrap();
    let status = r.status().as_u16();
    assert_error(status, &r.json().await.unwrap(), 405, "METHOD_NOT_ALLOWED");

    // Pages outside the API keep their plain answers.
    let r = t.client.get(t.url("/no-such-page")).send().await.unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(r.text().await.unwrap(), "Not found.");
}

#[tokio::test]
async fn concurrent_creates_with_one_idempotency_key_give_one_payment() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let body = json!({ "price_amount": 10, "price_currency": "usd", "pay_currency": "eth", "idempotency_key": "race-1" });
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let req = t.client.post(t.url("/v1/payment")).header("x-api-key", &key).json(&body);
        tasks.spawn(async move {
            let r = req.send().await.unwrap();
            (r.status().as_u16(), r.json::<Value>().await.unwrap())
        });
    }
    let mut ids = Vec::new();
    while let Some(res) = tasks.join_next().await {
        let (status, p) = res.unwrap();
        assert_eq!(status, 201, "{p}");
        ids.push(common::pid(&p));
    }
    ids.dedup();
    assert_eq!(ids.len(), 1, "{ids:?}");
    let (_, list) = t.get("/v1/payment", &key).await;
    assert_eq!(list["total"], 1);
}
