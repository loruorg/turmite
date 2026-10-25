mod common;

use serde_json::json;

#[tokio::test]
async fn payment_lifecycle_through_the_api() {
    let t = common::start(Default::default()).await;
    let (_pid, key) = t.project().await;

    let (status, body) = t.get("/v1/status", "").await;
    assert_eq!((status, body["message"].as_str()), (200, Some("OK")));

    let (status, p) = t
        .post("/v1/payment", &key, &json!({ "price_amount": 100, "price_currency": "usd", "pay_currency": "btc", "order_id": "A-1" }))
        .await;
    assert_eq!(status, 201, "{p}");
    assert_eq!(p["payment_status"], "waiting");
    assert_eq!(p["pay_amount"], json!(0.001));
    assert_eq!(p["price_amount"], json!(100));
    let id = common::pid(&p);

    t.deposit(id, "0.001", 0).await;
    let (_, p) = t.get(&format!("/v1/payment/{id}"), &key).await;
    assert_eq!(p["payment_status"], "confirming");

    t.deposit(id, "0.000001", 5).await; // a second, confirmed deposit
    let (_, p) = t.get(&format!("/v1/payment/{id}"), &key).await;
    assert_eq!(p["payment_status"], "confirming", "the first deposit is not confirmed yet");

    // Unknown key, unknown currency.
    let (status, _) = t.get(&format!("/v1/payment/{id}"), "tm_wrong").await;
    assert_eq!(status, 401);
    let (status, e) = t.post("/v1/payment", &key, &json!({ "price_amount": 1, "price_currency": "usd", "pay_currency": "doge" })).await;
    assert_eq!(status, 400);
    assert_eq!(e["code"], "INVALID_REQUEST_PARAMS");
}
