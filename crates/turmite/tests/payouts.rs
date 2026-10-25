//! Payouts on mock chains: API batches with a bearer token, limits, idempotency, the emailed-code approval,
//! reject, cancel, resolve, webhooks and the dashboard pages.

mod common;

use rust_decimal::Decimal;
use serde_json::{Value, json};
use turmite::{
    auth::totp,
    crypto::sha256_hex,
    error::Error,
    hot,
    payouts::{
        PayoutStatus,
        limits::PayoutLimits,
        service::{self, NewWithdrawal, Source},
    },
};

const PASSWORD: &str = "a long enough password";
const EVM_TO: &str = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";

struct Ctx {
    pid: i64,
    key: String,
    user_id: i64,
    secret: Vec<u8>,
    jwt: String,
}

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

async fn user(t: &common::TestApp, pid: i64, email: &str, role: &str) -> (i64, Vec<u8>) {
    let store = &t.app.store;
    let hash = turmite::auth::hash_password(PASSWORD).await.unwrap();
    let uid = store.create_user(email, &hash, "member").await.unwrap();
    store.set_member(pid, uid, role).await.unwrap();
    let secret = totp::new_secret();
    store.set_pending_totp(uid, &t.app.sealer.seal("totp", &secret)).await.unwrap();
    store.enable_totp(uid, 0).await.unwrap();
    (uid, secret)
}

async fn jwt(t: &common::TestApp, email: &str) -> String {
    let r = t.client.post(t.url("/v1/auth")).json(&json!({ "email": email, "password": PASSWORD })).send().await.unwrap();
    assert_eq!(r.status(), 200);
    r.json::<Value>().await.unwrap()["token"].as_str().unwrap().to_string()
}

/// A project with a project admin (TOTP on), a hot wallet per family and a bearer token.
async fn setup(t: &common::TestApp) -> Ctx {
    let (pid, key) = t.project().await;
    let (user_id, secret) = user(t, pid, "admin@shop.test", "project_admin").await;
    for family in ["evm", "tron", "solana", "ton"] {
        let s = hot::generate_secret(family).unwrap();
        let address = hot::derive_address(family, &s, true).unwrap();
        let id = hot::store(&t.app, family, &format!("{family} hot"), &address, &s, None).await.unwrap();
        hot::assign(&t.app, pid, family, id).await.unwrap();
    }
    let jwt = jwt(t, "admin@shop.test").await;
    Ctx { pid, key, user_id, secret, jwt }
}

/// A fresh TOTP code for the user (codes work once; this test clock allows reuse of the step).
async fn code(t: &common::TestApp, user_id: i64, secret: &[u8]) -> String {
    sqlx::query("UPDATE users SET totp_last_step = 0 WHERE user_id = $1").bind(user_id).execute(&t.app.store.pool).await.unwrap();
    totp::code(secret, chrono::Utc::now().timestamp() as u64)
}

async fn api(t: &common::TestApp, c: &Ctx, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let mut req = match method {
        "GET" => t.client.get(t.url(path)),
        _ => t.client.post(t.url(path)),
    }
    .header("x-api-key", &c.key)
    .header("authorization", format!("Bearer {}", c.jwt));
    if let Some(b) = body {
        req = req.json(&b);
    }
    let r = req.send().await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

async fn limits(t: &common::TestApp, pid: i64, min: &str, auto: &str, hourly: &str, daily: &str) {
    let l = PayoutLimits { min_usd: d(min), auto_approve_usd: d(auto), hourly_usd: d(hourly), daily_usd: d(daily) };
    t.app.store.set_payout_limits(pid, &l).await.unwrap();
}

async fn deliveries(t: &common::TestApp, event: &str) -> Vec<Value> {
    sqlx::query_scalar("SELECT payload FROM webhook_deliveries WHERE event_type = $1 ORDER BY webhook_delivery_id")
        .bind(event)
        .fetch_all(&t.app.store.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn api_batch_create_verify_and_list() {
    let t = common::start(Default::default()).await;
    let c = setup(&t).await;
    let eth = t.app.store.network_by_chain("ethereum").await.unwrap().unwrap();
    limits(&t, c.pid, "1", "100", "0", "0").await;
    let (_, stored) =
        service::add_address(&t.app, c.pid, eth.network_id, "Supplier", &EVM_TO.to_lowercase(), None, c.user_id).await.unwrap();
    assert_eq!(stored, EVM_TO, "stored in EIP-55 form");

    // Bearer token required.
    let r = t.client.post(t.url("/v1/payout")).header("x-api-key", &c.key).json(&json!({ "withdrawals": [] })).send().await.unwrap();
    assert_eq!(r.status(), 401);

    // Only address-book destinations.
    let (status, e) = api(
        &t,
        &c,
        "POST",
        "/v1/payout",
        Some(json!({ "withdrawals": [{ "address": "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266", "currency": "eth", "amount": "0.01" }] })),
    )
    .await;
    assert_eq!(status, 400, "{e}");
    assert!(e["message"].as_str().unwrap().contains("address book"), "{e}");

    // A wrong EIP-55 checksum is refused.
    let (status, _) = api(
        &t,
        &c,
        "POST",
        "/v1/payout",
        Some(json!({ "withdrawals": [{ "address": "0x70997970c51812dc3A010C7d01b50e0d17dc79C8", "currency": "eth", "amount": "0.01" }] })),
    )
    .await;
    assert_eq!(status, 400);

    // No payouts on Bitcoin; minimum payout.
    let (status, e) = api(
        &t,
        &c,
        "POST",
        "/v1/payout",
        Some(json!({ "withdrawals": [{ "address": "bcrt1qxyz", "currency": "btc", "amount": "0.01" }] })),
    )
    .await;
    assert_eq!(status, 400);
    assert!(e["message"].as_str().unwrap().contains("Bitcoin"), "{e}");
    let (status, e) =
        api(&t, &c, "POST", "/v1/payout", Some(json!({ "withdrawals": [{ "address": EVM_TO, "currency": "eth", "amount": "0.0001" }] })))
            .await;
    assert_eq!(status, 400);
    assert!(e["message"].as_str().unwrap().contains("minimum"), "{e}");

    // 0.01 ETH = 40 USD (auto-approve), 0.05 ETH = 200 USD (needs approval).
    let (url, _) = ("https://shop.test/payout-ipn".to_string(), ());
    let body = json!({
        "ipn_callback_url": url,
        "withdrawals": [
            { "address": EVM_TO.to_lowercase(), "currency": "eth", "amount": "0.01", "unique_external_id": "r-1" },
            { "address": EVM_TO, "currency": "eth", "amount": 0.05, "unique_external_id": "r-2", "payout_description": "Refund" },
        ]
    });
    let (status, batch) = api(&t, &c, "POST", "/v1/payout", Some(body.clone())).await;
    assert_eq!(status, 201, "{batch}");
    let batch_id = batch["id"].as_str().unwrap().to_string();
    let w = batch["withdrawals"].as_array().unwrap();
    assert_eq!(w.len(), 2);
    assert_eq!(w[0]["status"], "pending-otp-verification");
    assert_eq!(w[0]["address"], EVM_TO);
    assert_eq!(w[0]["amount"], "0.01");
    assert_eq!(w[0]["currency"], "eth");
    assert_eq!(w[0]["batch_withdrawal_id"], batch_id);
    assert_eq!(w[1]["payout_description"], "Refund");
    assert_eq!(w[1]["ipn_callback_url"], url);

    // Idempotency: the same request returns the same payouts; other fields with a used id give 409.
    let (status, again) = api(&t, &c, "POST", "/v1/payout", Some(body)).await;
    assert_eq!(status, 201);
    assert_eq!(again["id"], batch_id);
    assert_eq!(again["withdrawals"][1]["id"], w[1]["id"]);
    let (status, _) = api(
        &t,
        &c,
        "POST",
        "/v1/payout",
        Some(json!({ "withdrawals": [{ "address": EVM_TO, "currency": "eth", "amount": "0.02", "unique_external_id": "r-1" }] })),
    )
    .await;
    assert_eq!(status, 409);

    // Get by batch id with the API key only.
    let (status, got) = t.get(&format!("/v1/payout/{batch_id}"), &c.key).await;
    assert_eq!(status, 200);
    assert_eq!(got["withdrawals"].as_array().unwrap().len(), 2);

    // Verify: a wrong code, then the right one.
    let (status, _) = api(&t, &c, "POST", &format!("/v1/payout/{batch_id}/verify"), Some(json!({ "verification_code": "000000" }))).await;
    assert_eq!(status, 400);
    let good = code(&t, c.user_id, &c.secret).await;
    let (status, verified) =
        api(&t, &c, "POST", &format!("/v1/payout/{batch_id}/verify"), Some(json!({ "verification_code": good }))).await;
    assert_eq!(status, 200, "{verified}");
    assert_eq!(verified["withdrawals"][0]["status"], "pending");
    assert_eq!(verified["withdrawals"][1]["status"], "pending-approval");
    let id1: i64 = verified["withdrawals"][1]["id"].as_str().unwrap().parse().unwrap();
    let p = t.app.store.payout(c.pid, id1).await.unwrap().unwrap();
    assert_eq!(p.approval_reason.as_deref(), Some("above-auto-approve"));
    assert_eq!(p.verified_by, Some(c.user_id));

    // Webhooks for the status changes, to the withdrawal's IPN URL.
    let pending = deliveries(&t, "payout.pending").await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["status"], "pending");
    assert_eq!(deliveries(&t, "payout.pending-approval").await.len(), 1);

    // List with filters and pages.
    let (status, list) = api(&t, &c, "GET", "/v1/payout?limit=1&page=0", None).await;
    assert_eq!(status, 200, "{list}");
    assert_eq!((list["total"].as_i64(), list["pagesCount"].as_i64(), list["payouts"].as_array().unwrap().len()), (Some(2), Some(2), 1));
    let (_, list) = api(&t, &c, "GET", "/v1/payout?status=pending-approval", None).await;
    assert_eq!(list["total"], 2 - 1);
    let (status, _) = api(&t, &c, "GET", "/v1/payout?status=finished", None).await;
    assert_eq!(status, 400);

    // A read-only member cannot create payouts; another project's key cannot see the batch.
    user(&t, c.pid, "viewer@shop.test", "read_only").await;
    let viewer = Ctx { jwt: jwt(&t, "viewer@shop.test").await, ..c };
    let (status, _) = api(
        &t,
        &viewer,
        "POST",
        "/v1/payout",
        Some(json!({ "withdrawals": [{ "address": EVM_TO, "currency": "eth", "amount": "0.01" }] })),
    )
    .await;
    assert_eq!(status, 403);
    let (status, _) = api(&t, &viewer, "GET", "/v1/payout", None).await;
    assert_eq!(status, 200);
    let (_, other_key) = t.project().await;
    let (status, _) = t.get(&format!("/v1/payout/{batch_id}"), &other_key).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn dashboard_approval_needs_the_emailed_code() {
    let t = common::start(Default::default()).await;
    let c = setup(&t).await;
    let (other, _) = user(&t, c.pid, "second@shop.test", "project_admin").await;
    limits(&t, c.pid, "0", "100", "0", "0").await;
    let admin = t.app.store.user(c.user_id).await.unwrap();
    let new = |amount: &str| NewWithdrawal { address: EVM_TO.into(), currency: "eth".into(), amount: d(amount), ..Default::default() };
    let src = Source::Dashboard { user_id: c.user_id };

    let out = service::create_batch(&t.app, c.pid, src, vec![new("0.05")], None).await.unwrap();
    let id = out.payouts[0].payout_id;
    assert_eq!(out.payouts[0].status, PayoutStatus::PendingApproval);

    // Step 1: a code for this admin, stored as a keyed hash.
    let code1 = service::start_approval(&t.app, c.pid, id, &admin).await.unwrap();
    let p = t.app.store.payout(c.pid, id).await.unwrap().unwrap();
    assert_eq!(p.status, PayoutStatus::PendingOtpVerification);
    let stored = t.app.store.approval_code(id).await.unwrap().unwrap();
    assert_ne!(stored.code_hash, code1);
    assert!(!stored.code_hash.contains(&code1));

    // Another admin cannot use it.
    let e = service::confirm_approval(&t.app, c.pid, id, other, &code1).await.unwrap_err();
    assert!(matches!(e, Error::Forbidden(_)), "{e}");

    // 5 wrong attempts use it up; the payout waits for approval again.
    let wrong = if code1 == "000000" { "111111" } else { "000000" };
    for left in (1..=4).rev() {
        let e = service::confirm_approval(&t.app, c.pid, id, c.user_id, wrong).await.unwrap_err();
        assert!(e.to_string().contains(&format!("{left} attempt")), "{e}");
    }
    let e = service::confirm_approval(&t.app, c.pid, id, c.user_id, wrong).await.unwrap_err();
    assert!(e.to_string().contains("no attempts"), "{e}");
    assert_eq!(t.app.store.payout(c.pid, id).await.unwrap().unwrap().status, PayoutStatus::PendingApproval);
    assert!(service::confirm_approval(&t.app, c.pid, id, c.user_id, &code1).await.is_err(), "the old code is gone");

    // An expired code also ends the step.
    let code2 = service::start_approval(&t.app, c.pid, id, &admin).await.unwrap();
    sqlx::query("UPDATE payout_approval_codes SET expires_at = now() - interval '1 second'").execute(&t.app.store.pool).await.unwrap();
    let e = service::confirm_approval(&t.app, c.pid, id, c.user_id, &code2).await.unwrap_err();
    assert!(e.to_string().contains("expired"), "{e}");
    assert_eq!(t.app.store.payout(c.pid, id).await.unwrap().unwrap().status, PayoutStatus::PendingApproval);

    // The worker also returns expired codes to approval.
    service::start_approval(&t.app, c.pid, id, &admin).await.unwrap();
    sqlx::query("UPDATE payout_approval_codes SET expires_at = now() - interval '1 second'").execute(&t.app.store.pool).await.unwrap();
    assert_eq!(t.app.store.expire_approval_codes().await.unwrap().len(), 1);
    assert_eq!(t.app.store.payout(c.pid, id).await.unwrap().unwrap().status, PayoutStatus::PendingApproval);

    // The right code from the right admin: pending.
    let code3 = service::start_approval(&t.app, c.pid, id, &admin).await.unwrap();
    let p = service::confirm_approval(&t.app, c.pid, id, c.user_id, &code3).await.unwrap();
    assert_eq!(p.status, PayoutStatus::Pending);
    assert_eq!(p.approved_by, Some(c.user_id));
    assert!(p.approved_at.is_some());
    assert!(t.app.store.approval_code(id).await.unwrap().is_none());
    assert!(service::start_approval(&t.app, c.pid, id, &admin).await.is_err(), "nothing to approve now");

    // Cancel works before signing; reject only while it waits for a person.
    assert!(service::reject(&t.app, c.pid, id).await.is_err());
    assert_eq!(service::cancel(&t.app, c.pid, id).await.unwrap().status, PayoutStatus::Cancelled);
    assert!(service::cancel(&t.app, c.pid, id).await.is_err());
    let r = service::create_batch(&t.app, c.pid, src, vec![new("0.06")], None).await.unwrap().payouts[0].payout_id;
    assert_eq!(service::reject(&t.app, c.pid, r).await.unwrap().status, PayoutStatus::Rejected);

    // A payout in signing cannot be cancelled.
    let s = service::create_batch(&t.app, c.pid, src, vec![new("0.01")], None).await.unwrap().payouts[0].payout_id;
    sqlx::query("UPDATE payouts SET status = 'signing' WHERE payout_id = $1").bind(s).execute(&t.app.store.pool).await.unwrap();
    assert!(matches!(service::cancel(&t.app, c.pid, s).await.unwrap_err(), Error::Conflict(_)));

    // needs-review: resolved once, with a note, final webhook.
    sqlx::query("UPDATE payouts SET status = 'needs-review' WHERE payout_id = $1").bind(s).execute(&t.app.store.pool).await.unwrap();
    assert!(service::resolve(&t.app, c.pid, s, false, " ").await.is_err(), "a note is required");
    let p = service::resolve(&t.app, c.pid, s, false, "No transaction with this nonce on the explorer.").await.unwrap();
    assert_eq!(p.status, PayoutStatus::Failed);
    assert!(service::resolve(&t.app, c.pid, s, true, "again").await.is_err());
    let finals: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM webhook_deliveries WHERE event_type = 'payout.failed' AND is_final")
        .fetch_one(&t.app.store.pool)
        .await
        .unwrap();
    // No endpoint and no IPN URL: nothing to deliver. Add an endpoint and check the event shape.
    assert_eq!(finals, 0);
}

#[tokio::test]
async fn window_limits_count_in_flight_payouts() {
    let t = common::start(Default::default()).await;
    let c = setup(&t).await;
    limits(&t, c.pid, "0", "100", "150", "1000").await;
    let src = Source::Dashboard { user_id: c.user_id };
    let w = |amount: &str| NewWithdrawal { address: EVM_TO.into(), currency: "eth".into(), amount: d(amount), ..Default::default() };
    // 0.02 ETH = 80 USD each. The second one takes the hour over 150.
    let a = service::create_batch(&t.app, c.pid, src, vec![w("0.02")], None).await.unwrap().payouts.remove(0);
    assert_eq!(a.status, PayoutStatus::Pending);
    sqlx::query("UPDATE payouts SET status = 'initiated' WHERE payout_id = $1").bind(a.payout_id).execute(&t.app.store.pool).await.unwrap();
    let b = service::create_batch(&t.app, c.pid, src, vec![w("0.02")], None).await.unwrap().payouts.remove(0);
    assert_eq!(b.status, PayoutStatus::PendingApproval);
    assert_eq!(b.approval_reason.as_deref(), Some("hourly-limit"));
    // A failed payout does not count.
    sqlx::query("UPDATE payouts SET status = 'failed' WHERE payout_id = $1").bind(a.payout_id).execute(&t.app.store.pool).await.unwrap();
    let c2 = service::create_batch(&t.app, c.pid, src, vec![w("0.02")], None).await.unwrap().payouts.remove(0);
    assert_eq!(c2.status, PayoutStatus::Pending);
    // Several withdrawals of one batch count each other.
    let many = service::create_batch(&t.app, c.pid, src, vec![w("0.01"), w("0.01"), w("0.01")], None).await.unwrap();
    let st: Vec<_> = many.payouts.iter().map(|p| p.status).collect();
    assert_eq!(st, vec![PayoutStatus::Pending, PayoutStatus::PendingApproval, PayoutStatus::PendingApproval]);
}

#[tokio::test]
async fn webhooks_go_to_project_endpoints() {
    let t = common::start(Default::default()).await;
    let c = setup(&t).await;
    t.app.store.add_webhook_endpoint(c.pid, "https://shop.test/hook", &t.app.sealer.seal_str("webhook_secret", "s")).await.unwrap();
    limits(&t, c.pid, "0", "0", "0", "0").await;
    let admin = t.app.store.user(c.user_id).await.unwrap();
    let w = NewWithdrawal {
        address: EVM_TO.into(),
        currency: "eth".into(),
        amount: d("0.01"),
        ipn_callback_url: Some("https://shop.test/ipn".into()),
        ..Default::default()
    };
    let p = service::create_batch(&t.app, c.pid, Source::Dashboard { user_id: c.user_id }, vec![w], None).await.unwrap().payouts.remove(0);
    let created = deliveries(&t, "payout.pending-approval").await;
    assert_eq!(created.len(), 2, "a dashboard payout sends its first webhook at once: endpoint and IPN URL");
    let code = service::start_approval(&t.app, c.pid, p.payout_id, &admin).await.unwrap();
    service::confirm_approval(&t.app, c.pid, p.payout_id, c.user_id, &code).await.unwrap();
    let rows: Vec<(Option<i64>, Value, bool)> =
        sqlx::query_as("SELECT webhook_endpoint_id, payload, is_final FROM webhook_deliveries WHERE event_type = 'payout.pending'")
            .fetch_all(&t.app.store.pool)
            .await
            .unwrap();
    assert_eq!(rows.len(), 2, "endpoint and IPN URL");
    let envelope = rows.iter().find(|r| r.0.is_some()).unwrap();
    assert_eq!(envelope.1["event"], "payout.pending");
    assert_eq!(envelope.1["data"]["id"], p.payout_id.to_string());
    let flat = rows.iter().find(|r| r.0.is_none()).unwrap();
    assert_eq!(flat.1["status"], "pending");
    assert!(!flat.2);
    service::cancel(&t.app, c.pid, p.payout_id).await.unwrap();
    let finals: Vec<bool> = sqlx::query_scalar("SELECT is_final FROM webhook_deliveries WHERE event_type = 'payout.cancelled'")
        .fetch_all(&t.app.store.pool)
        .await
        .unwrap();
    assert_eq!(finals, vec![true, true]);
}

#[tokio::test]
async fn dashboard_pages_render_without_javascript() {
    let t = common::start(Default::default()).await;
    let c = setup(&t).await;
    sqlx::query("UPDATE users SET role = 'admin' WHERE user_id = $1").bind(c.user_id).execute(&t.app.store.pool).await.unwrap();
    let token = "payout-test-session";
    t.app
        .store
        .create_session(&sha256_hex(token.as_bytes()), c.user_id, None, None, chrono::Utc::now() + chrono::Duration::hours(1))
        .await
        .unwrap();
    let cookie = format!("turmite_session={token}");
    let get = |path: String| {
        let cookie = cookie.clone();
        let t = &t;
        async move {
            let r = t.client.get(t.url(&path)).header("cookie", cookie).send().await.unwrap();
            (r.status().as_u16(), r.text().await.unwrap())
        }
    };
    let post = |path: String, form: Vec<(&'static str, String)>| {
        let cookie = cookie.clone();
        let t = &t;
        async move {
            let r = t.client.post(t.url(&path)).header("cookie", cookie).header("origin", t.base.clone()).form(&form).send().await.unwrap();
            (r.status().as_u16(), r.headers().get("set-cookie").map(|v| v.to_str().unwrap().to_string()).unwrap_or_default())
        }
    };
    let base = format!("/dashboard/p/{}", c.pid);
    let eth = t.app.store.network_by_chain("ethereum").await.unwrap().unwrap();

    // Address book: a bad checksum is refused, a good address is added.
    let (s, flash) = post(
        format!("{base}/payouts/address-book"),
        vec![
            ("network", eth.network_id.to_string()),
            ("label", "Bad".into()),
            ("address", "0x70997970c51812dc3A010C7d01b50e0d17dc79C8".into()),
            ("totp", code(&t, c.user_id, &c.secret).await),
        ],
    )
    .await;
    assert_eq!(s, 303);
    assert!(flash.starts_with("turmite_flash=error"), "{flash}");
    let (s, flash) = post(
        format!("{base}/payouts/address-book"),
        vec![
            ("network", eth.network_id.to_string()),
            ("label", "Supplier".into()),
            ("address", EVM_TO.into()),
            ("totp", code(&t, c.user_id, &c.secret).await),
        ],
    )
    .await;
    assert_eq!(s, 303);
    assert!(flash.starts_with("turmite_flash=ok"), "{flash}");
    // Limits need a two-factor code.
    let (_, flash) = post(
        format!("{base}/payouts/limits"),
        vec![
            ("min_usd", "1".into()),
            ("auto_approve_usd", "50".into()),
            ("hourly_usd", "0".into()),
            ("daily_usd", "0".into()),
            ("totp", "000000".into()),
        ],
    )
    .await;
    assert!(flash.starts_with("turmite_flash=error"), "{flash}");
    let (_, flash) = post(
        format!("{base}/payouts/limits"),
        vec![
            ("min_usd", "1".into()),
            ("auto_approve_usd", "50".into()),
            ("hourly_usd", "0".into()),
            ("daily_usd", "0".into()),
            ("totp", code(&t, c.user_id, &c.secret).await),
        ],
    )
    .await;
    assert!(flash.starts_with("turmite_flash=ok"), "{flash}");
    assert_eq!(t.app.store.payout_limits(c.pid).await.unwrap().auto_approve_usd, d("50"));

    // A new payout from the form, then the approval form on its page.
    let (s, _) = post(
        format!("{base}/payouts"),
        vec![
            ("currency", "eth".into()),
            ("address", EVM_TO.into()),
            ("amount", "0.05".into()),
            ("totp", code(&t, c.user_id, &c.secret).await),
        ],
    )
    .await;
    assert_eq!(s, 303);
    let (rows, _) = t.app.store.list_payouts(c.pid, &Default::default(), turmite::db::Page::new(10, 0)).await.unwrap();
    let id = rows[0].payout_id;
    assert_eq!(rows[0].status, PayoutStatus::PendingApproval);
    let (_, flash) = post(format!("{base}/payouts/{id}/approve"), vec![("totp", code(&t, c.user_id, &c.secret).await)]).await;
    assert!(flash.contains("A+code+was+sent+to"), "{flash}");
    let (s, page) = get(format!("{base}/payouts/{id}")).await;
    assert_eq!(s, 200);
    assert!(page.contains("Code from the email"), "the code step is shown");

    for path in [
        format!("{base}/payouts"),
        format!("{base}/payouts?status=pending-otp-verification"),
        format!("{base}/payouts/new"),
        format!("{base}/payouts/address-book"),
        format!("{base}/payouts/limits"),
        format!("{base}/payouts/{id}"),
        "/dashboard/admin/hot-wallets".to_string(),
    ] {
        let (s, html) = get(path.clone()).await;
        assert_eq!(s, 200, "{path}");
        assert!(!html.contains("<script>"), "no inline script on {path}");
    }
    let (_, html) = get("/dashboard/admin/hot-wallets".into()).await;
    assert!(html.contains("Bitcoin and Monero have no hot wallet"));

    // Generate and assign a hot wallet through the forms.
    let (_, flash) = post(
        "/dashboard/admin/hot-wallets/generate".into(),
        vec![("family", "solana".into()), ("label", "Second".into()), ("totp", code(&t, c.user_id, &c.secret).await)],
    )
    .await;
    assert!(flash.starts_with("turmite_flash=ok"), "{flash}");
    let (_, flash) = post(
        "/dashboard/admin/hot-wallets/import".into(),
        vec![
            ("family", "evm".into()),
            ("label", "Imported".into()),
            ("secret", "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d".into()),
            ("totp", code(&t, c.user_id, &c.secret).await),
        ],
    )
    .await;
    assert!(flash.starts_with("turmite_flash=ok"), "{flash}");
    let w = hot::list(&t.app).await.unwrap().into_iter().find(|w| w.label == "Imported").unwrap();
    assert_eq!(w.address, EVM_TO, "anvil account 1");
    let (_, flash) = post(
        "/dashboard/admin/hot-wallets/assign".into(),
        vec![
            ("project", c.pid.to_string()),
            ("family", "evm".into()),
            ("wallet", w.hot_wallet_id.to_string()),
            ("totp", code(&t, c.user_id, &c.secret).await),
        ],
    )
    .await;
    assert!(flash.starts_with("turmite_flash=ok"), "{flash}");
    assert_eq!(hot::address_for_project(&t.app, c.pid, "evm").await.unwrap().unwrap().1, EVM_TO);
}

async fn next_nonce(t: &common::TestApp, network_id: i64, address: &str, pending: u64) -> u64 {
    let mut conn = t.app.store.pool.acquire().await.unwrap();
    turmite::db::PgStore::next_evm_nonce(&mut conn, network_id, address, pending).await.unwrap()
}

#[tokio::test]
async fn evm_sweeps_and_payouts_take_nonces_from_one_source() {
    let t = common::start(Default::default()).await;
    let c = setup(&t).await;
    limits(&t, c.pid, "0", "1000000", "0", "0").await;
    let store = &t.app.store;
    let eth = store.list_networks().await.unwrap().into_iter().find(|n| n.chain == "ethereum").unwrap();
    let hot = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";
    assert_eq!(next_nonce(&t, eth.network_id, hot, 3).await, 3, "nothing stored: the node's pending nonce");

    // A flush that may still land holds nonce 5.
    sqlx::query(
        "INSERT INTO evm_txs (network_id, kind, factory, cold_wallet, from_address, nonce, txid, status) \
         VALUES ($1, 'flush', '0xf', '0xc', $2, 5, '0x01', 'broadcast')",
    )
    .bind(eth.network_id)
    .bind(hot)
    .execute(&store.pool)
    .await
    .unwrap();
    assert_eq!(next_nonce(&t, eth.network_id, &hot.to_lowercase(), 3).await, 6);

    // A payout that a node refused for now keeps nonce 7 for its retry.
    let w = NewWithdrawal { address: EVM_TO.into(), currency: "eth".into(), amount: d("0.01"), ..Default::default() };
    let out = service::create_batch(&t.app, c.pid, Source::Dashboard { user_id: c.user_id }, vec![w], None).await.unwrap();
    let payout_id = out.payouts[0].payout_id;
    sqlx::query("UPDATE payouts SET status = 'signing' WHERE payout_id = $1").bind(payout_id).execute(&store.pool).await.unwrap();
    sqlx::query(
        "INSERT INTO payout_attempts (payout_id, network_id, from_address, txid, raw_tx, nonce) VALUES ($1, $2, $3, '0x02', '0x', 7)",
    )
    .bind(payout_id)
    .bind(eth.network_id)
    .bind(hot)
    .execute(&store.pool)
    .await
    .unwrap();
    assert_eq!(next_nonce(&t, eth.network_id, hot, 3).await, 8, "the sweeper does not take the payout's nonce");
    assert_eq!(next_nonce(&t, eth.network_id, hot, 12).await, 12, "the node is ahead");

    // Final ones do not hold a nonce.
    sqlx::query("UPDATE payouts SET status = 'failed' WHERE payout_id = $1").bind(payout_id).execute(&store.pool).await.unwrap();
    sqlx::query("UPDATE evm_txs SET status = 'confirmed'").execute(&store.pool).await.unwrap();
    assert_eq!(next_nonce(&t, eth.network_id, hot, 3).await, 3);
}
