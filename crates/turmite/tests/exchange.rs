//! Pay with any coin, against a local stand-in for the exchange API. Swap statuses are shown;
//! the payment is credited only by the chain watcher.

mod common;

use reqwest::Response;
use serde_json::{Value, json};
use turmite::exchange::stub::Stub;

async fn admin_cookie(t: &common::TestApp) -> String {
    let uid = t.app.store.create_user("root@shop.example.com", "unused", "root").await.unwrap();
    let token = turmite::crypto::random_hex(24);
    let hash = turmite::crypto::sha256_hex(token.as_bytes());
    t.app.store.create_session(&hash, uid, None, None, chrono::Utc::now() + chrono::Duration::hours(1)).await.unwrap();
    format!("turmite_session={token}")
}

fn flash(r: &Response) -> String {
    r.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("turmite_flash="))
        .map(|v| url::form_urlencoded::parse(v.split(';').next().unwrap().as_bytes()).map(|(_, v)| v.into_owned()).collect())
        .unwrap_or_default()
}

async fn html(t: &common::TestApp, path: &str) -> String {
    let r = t.client.get(t.url(path)).send().await.unwrap();
    assert_eq!(r.status(), 200, "{path}");
    let body = r.text().await.unwrap();
    assert!(!body.to_ascii_lowercase().contains("<script"), "{path}: no script");
    body
}

async fn swap(t: &common::TestApp, token: &str, form: &[(&str, &str)]) -> Response {
    t.client.post(t.url(&format!("/x/{token}"))).form(form).send().await.unwrap()
}

fn token_of(p: &Value) -> String {
    p["payment_url"].as_str().unwrap().rsplit('/').next().unwrap().to_string()
}

/// A test app wired to a stub exchange, with the exchange on for a project.
async fn setup() -> (common::TestApp, Stub, i64, String) {
    let stub = Stub::start(false).await.unwrap();
    let t = common::start(common::TestOptions { exchange_base: Some(stub.base.clone()), ..Default::default() }).await;
    let (pid, key) = t.project().await;
    let cookie = admin_cookie(&t).await;
    let r = t
        .client
        .post(t.url(&format!("/dashboard/p/{pid}/settings/exchange")))
        .header("cookie", &cookie)
        .header("origin", &t.base)
        .form(&[("enabled", "1"), ("api_key", "partner-key"), ("markup", "1.65"), ("min_kyc", "A"), ("coins", ""), ("totp", "")])
        .send()
        .await
        .unwrap();
    assert!(flash(&r).starts_with("ok"), "{}", flash(&r));
    (t, stub, pid, key)
}

async fn usdt_payment(t: &common::TestApp, key: &str) -> Value {
    let (status, p) = t
        .post("/v1/payment", key, &json!({ "price_amount": 100, "price_currency": "usd", "pay_currency": "usdttrc20", "success_url": "https://shop.example.com/thanks" }))
        .await;
    assert_eq!(status, 201, "{p}");
    p
}

#[tokio::test]
async fn swap_flow_credits_only_what_arrives_on_chain() {
    let (t, stub, pid, key) = setup().await;
    assert!(!stub.requests("coins").is_empty(), "saving the key loads the coin list");
    let settings = t.app.store.exchange_settings(pid).await.unwrap();
    assert!(settings.enabled && settings.key_sealed.is_some());
    assert_ne!(settings.key_sealed.as_deref(), Some(b"partner-key".as_slice()), "the key is sealed");

    let p = usdt_payment(&t, &key).await;
    let token = token_of(&p);
    assert!(html(&t, &format!("/pay/{token}")).await.contains(&format!("href=\"/x/{token}\"")), "the payment page offers other coins");

    let list = html(&t, &format!("/x/{token}")).await;
    assert!(list.contains("Litecoin") && list.contains("Monero") && list.contains("100") && list.contains("USDT"));
    assert!(!list.contains("coin=usdt%3ATRC20"), "the payment's own coin is not offered");
    let found = html(&t, &format!("/x/{token}?q=lite")).await;
    assert!(found.contains("Litecoin") && !found.contains("Monero"), "search by name");
    let chosen = html(&t, &format!("/x/{token}?coin=ltc%3AMainnet")).await;
    assert!(chosen.contains("name=\"refund\"") && chosen.contains("Get the deposit address") && chosen.contains("KYC rating A"));

    // A coin with a memo needs a refund address.
    let r = swap(&t, &token, &[("coin", "xrp:Mainnet"), ("refund", "")]).await;
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("needs a refund address"));

    let r = swap(&t, &token, &[("coin", "ltc:Mainnet"), ("refund", "ltc1qrefundaddress0000000000000000000000")]).await;
    assert_eq!(r.status(), 303);
    let trade_path = r.headers()["location"].to_str().unwrap().to_string();
    assert!(trade_path.starts_with("/x/") && trade_path != format!("/x/{token}"));

    let rate = &stub.requests("new_rate")[0];
    for (k, v) in [
        ("ticker_from", "ltc"),
        ("network_from", "Mainnet"),
        ("ticker_to", "usdt"),
        ("network_to", "TRC20"),
        ("amount_to", "100"),
        ("payment", "True"),
        ("min_kycrating", "A"),
        ("markup", "1.65"),
    ] {
        assert_eq!(rate.get(k).map(String::as_str), Some(v), "new_rate {k}");
    }
    let trade = &stub.requests("new_trade")[0];
    for (k, v) in [
        ("address", p["pay_address"].as_str().unwrap()),
        ("address_memo", "0"),
        ("fixed", "True"),
        ("payment", "True"),
        ("provider", "Quarry"),
        ("amount_to", "100"),
        ("refund", "ltc1qrefundaddress0000000000000000000000"),
    ] {
        assert_eq!(trade.get(k).map(String::as_str), Some(v), "new_trade {k}");
    }
    assert!(trade["id"].starts_with('R') && trade["webhook"].contains("/x/hook/") && trade["webhook_key"].len() >= 32);

    let page = html(&t, &trade_path).await;
    assert!(page.contains("ltc1demo") && page.contains("Quarry") && page.contains("KYC rating") && page.contains("1 LTC = "));
    assert!(page.contains("Send exactly") && page.contains(&format!("src=\"{trade_path}/status\"")));
    let frame = html(&t, &format!("{trade_path}/status")).await;
    assert!(frame.contains("Waiting for your LTC") && frame.contains("http-equiv=\"refresh\"") && frame.contains("animation-duration:"));

    // The exchange says it finished: shown, but the payment is not credited.
    let row = t.app.store.trades_for_payment(common::pid(&p)).await.unwrap().remove(0);
    stub.set_status(&row.trade_id, "finished");
    turmite::exchange::poll(&t.app).await;
    assert_eq!(t.app.store.trade_by_token(&row.token).await.unwrap().unwrap().status, "finished");
    let id = common::pid(&p);
    assert_eq!(t.app.store.payment_any(id).await.unwrap().unwrap().status.as_str(), "waiting");
    let frame = html(&t, &format!("{trade_path}/status")).await;
    assert!(frame.contains("The exchange sent the coins"), "{frame}");

    // Coins arrive on chain: the watcher credits the payment.
    let net = t.app.store.network_by_chain("tron").await.unwrap().unwrap();
    t.deposit(id, "100", net.confirmations_required.max(1)).await;
    assert_eq!(t.app.store.payment_any(id).await.unwrap().unwrap().status.as_str(), "finished");
    let frame = html(&t, &format!("{trade_path}/status")).await;
    assert!(frame.contains("st-paid") && frame.contains("href=\"https://shop.example.com/thanks\""));

    // Webhook: the key must match; then the swap is fetched again.
    let hook = format!("/x/hook/{}", row.hook_secret);
    let r = t.client.post(t.url(&hook)).header("webhook-key", "wrong").body("{}").send().await.unwrap();
    assert_eq!(r.status(), 401);
    let r = t.client.post(t.url(&hook)).header("webhook-key", &row.webhook_key).body("{}").send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(t.client.post(t.url("/x/hook/unknown")).send().await.unwrap().status(), 404);
}

#[tokio::test]
async fn failed_swaps_offer_a_retry_and_bad_quotes_show_an_error() {
    let (t, stub, _pid, key) = setup().await;
    let p = usdt_payment(&t, &key).await;
    let token = token_of(&p);

    // The stub has no rate for this coin.
    let r = swap(&t, &token, &[("coin", "doge:Mainnet"), ("refund", "")]).await;
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("cannot do this swap"));
    assert!(stub.requests("new_trade").is_empty());

    let r = swap(&t, &token, &[("coin", "btc:Mainnet"), ("refund", "")]).await;
    let trade_path = r.headers()["location"].to_str().unwrap().to_string();
    let row = t.app.store.trades_for_payment(common::pid(&p)).await.unwrap().remove(0);
    assert!(!stub.requests("new_trade")[0].contains_key("refund"), "no refund address sent when none is given");
    for (status, title) in [("failed", "The swap failed"), ("expired", "The swap expired"), ("refunded", "The swap was refunded")] {
        stub.set_status(&row.trade_id, status);
        turmite::exchange::refresh_trade(&t.app, &t.app.store.trade_by_token(&row.token).await.unwrap().unwrap()).await.unwrap();
        let frame = html(&t, &format!("{trade_path}/status")).await;
        assert!(frame.contains(title), "{status}");
        assert!(frame.contains(&format!("href=\"/x/{token}\" target=\"_top\">Try again")) && frame.contains("Pay in USDT"));
        assert!(!frame.contains("http-equiv=\"refresh\""));
    }
    let list = html(&t, &format!("/x/{token}")).await;
    assert!(!list.contains("You started a swap"), "a closed swap is not offered again");
}

#[tokio::test]
async fn the_swap_webhook_uses_the_checkout_domain() {
    let (t, stub, pid, key) = setup().await;
    t.app.store.set_checkout_domain(pid, Some("pay.shop.example.com")).await.unwrap();
    let token = token_of(&usdt_payment(&t, &key).await);
    let r = swap(&t, &token, &[("coin", "ltc:Mainnet"), ("refund", "")]).await;
    assert_eq!(r.status(), 303);
    let hook = stub.requests("new_trade")[0]["webhook"].clone();
    assert!(hook.starts_with("https://pay.shop.example.com/x/hook/"), "{hook}");
}

#[tokio::test]
async fn a_swap_above_the_coin_maximum_is_refused() {
    let (t, stub, _pid, key) = setup().await;
    let body = json!({ "price_amount": 200_000, "price_currency": "usd", "pay_currency": "usdttrc20" });
    let token = token_of(&t.post("/v1/payment", &key, &body).await.1);
    let r = swap(&t, &token, &[("coin", "ltc:Mainnet"), ("refund", "")]).await;
    assert_eq!(r.status(), 200);
    let page = r.text().await.unwrap();
    assert!(page.contains("more than the exchange maximum") && page.contains("LTC"), "{page}");
    assert!(stub.requests("new_trade").is_empty());
}

#[tokio::test]
async fn exchange_off_or_without_a_stub_on_testnet() {
    let (t, _stub, pid, key) = setup().await;
    t.app.store.save_exchange_settings(pid, false, None, rust_decimal::Decimal::ZERO, "B", &[]).await.unwrap();
    let p = usdt_payment(&t, &key).await;
    let token = token_of(&p);
    assert!(!html(&t, &format!("/pay/{token}")).await.contains("/x/"));
    assert!(html(&t, &format!("/x/{token}")).await.contains("not open"));
    let r = swap(&t, &token, &[("coin", "ltc:Mainnet")]).await;
    assert!(r.text().await.unwrap().contains("does not take other coins"));

    // An allow list limits the coins.
    t.app.store.save_exchange_settings(pid, true, None, rust_decimal::Decimal::ZERO, "B", &["xmr".into()]).await.unwrap();
    let list = html(&t, &format!("/x/{token}")).await;
    assert!(list.contains("Monero") && !list.contains("Litecoin"));
    let r = swap(&t, &token, &[("coin", "ltc:Mainnet")]).await;
    assert!(r.text().await.unwrap().contains("does not take this coin"));

    // Testnet without a stub: the option never shows.
    let plain = common::start(Default::default()).await;
    let (ppid, pkey) = plain.project().await;
    let sealed = plain.app.sealer.seal_str("trocador_key", "k");
    plain.app.store.save_exchange_settings(ppid, true, Some(&sealed), rust_decimal::Decimal::ZERO, "B", &[]).await.unwrap();
    let p = usdt_payment(&plain, &pkey).await;
    assert!(!html(&plain, &format!("/pay/{}", token_of(&p))).await.contains("/x/"));
    assert!(!turmite::exchange::supported(&plain.app));
}

#[tokio::test]
async fn invoice_offers_any_coin_and_settles_in_a_stablecoin() {
    let (t, stub, _pid, key) = setup().await;
    let (_, inv) = t.post("/v1/invoice", &key, &json!({ "price_amount": 30, "price_currency": "usd" })).await;
    let path = url::Url::parse(inv["invoice_url"].as_str().unwrap()).unwrap().path().to_string();
    assert!(html(&t, &path).await.contains("value=\"any\""));
    let r = t.client.post(t.url(&path)).form(&[("currency", "any")]).send().await.unwrap();
    assert_eq!(r.status(), 303);
    let to = r.headers()["location"].to_str().unwrap().to_string();
    assert!(to.starts_with("/x/"), "{to}");
    let token = to.trim_start_matches("/x/");
    let p = t.app.store.payment_by_token(token).await.unwrap().unwrap();
    let an = t.app.store.asset_network(p.asset_network_id).await.unwrap();
    assert_eq!(an.ticker, "usdttrc20");
    let _ = stub;
}

#[tokio::test]
async fn a_wrong_key_is_refused_when_saved() {
    let stub = Stub::start(false).await.unwrap();
    let t = common::start(common::TestOptions { exchange_base: Some(stub.base.clone()), ..Default::default() }).await;
    let (pid, _key) = t.project().await;
    let cookie = admin_cookie(&t).await;
    let r = t
        .client
        .post(t.url(&format!("/dashboard/p/{pid}/settings/exchange")))
        .header("cookie", &cookie)
        .header("origin", &t.base)
        .form(&[("enabled", "1"), ("api_key", "bad-key"), ("markup", "1"), ("min_kyc", "B"), ("totp", "")])
        .send()
        .await
        .unwrap();
    assert!(flash(&r).starts_with("error"), "{}", flash(&r));
    assert!(t.app.store.exchange_settings(pid).await.unwrap().key_sealed.is_none());
    let r = t
        .client
        .post(t.url(&format!("/dashboard/p/{pid}/settings/exchange")))
        .header("cookie", &cookie)
        .header("origin", &t.base)
        .form(&[("enabled", "1"), ("api_key", ""), ("markup", "2"), ("min_kyc", "B"), ("totp", "")])
        .send()
        .await
        .unwrap();
    assert!(flash(&r).starts_with("error"), "markup must be 0, 1, 1.65 or 3");
    let page = t
        .client
        .get(t.url(&format!("/dashboard/p/{pid}/settings/exchange")))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("Trocador API key") && !page.contains("bad-key"));
}
