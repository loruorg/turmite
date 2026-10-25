//! Customer pages: no script, CSP, status frames, branding, return URLs, checkout domains, logos.

mod common;

use std::io::Cursor;

use reqwest::Response;
use serde_json::{Value, json};

/// A root user with a session. Returns the cookie header value.
async fn admin_cookie(t: &common::TestApp) -> String {
    let uid = t.app.store.create_user("root@shop.example.com", "unused", "root").await.unwrap();
    let token = turmite::crypto::random_hex(24);
    let hash = turmite::crypto::sha256_hex(token.as_bytes());
    t.app.store.create_session(&hash, uid, None, None, chrono::Utc::now() + chrono::Duration::hours(1)).await.unwrap();
    format!("turmite_session={token}")
}

async fn page(t: &common::TestApp, path: &str) -> (u16, String, reqwest::header::HeaderMap) {
    let r = t.client.get(t.url(path)).send().await.unwrap();
    let (status, headers) = (r.status().as_u16(), r.headers().clone());
    (status, r.text().await.unwrap(), headers)
}

async fn dash_post(t: &common::TestApp, cookie: &str, path: &str, form: &[(&str, &str)]) -> Response {
    t.client.post(t.url(path)).header("cookie", cookie).header("origin", &t.base).form(form).send().await.unwrap()
}

/// The flash message a dashboard redirect carries.
fn flash(r: &Response) -> String {
    r.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("turmite_flash="))
        .map(|v| url::form_urlencoded::parse(v.split(';').next().unwrap().as_bytes()).map(|(_, v)| v.into_owned()).collect())
        .unwrap_or_default()
}

async fn create_payment(t: &common::TestApp, key: &str, body: Value) -> Value {
    let mut b = json!({ "price_amount": 100, "price_currency": "usd", "pay_currency": "btc" });
    b.as_object_mut().unwrap().extend(body.as_object().unwrap().clone());
    let (status, p) = t.post("/v1/payment", key, &b).await;
    assert_eq!(status, 201, "{p}");
    p
}

fn path_of(url: &Value) -> String {
    let u = url::Url::parse(url.as_str().unwrap()).unwrap();
    u.path().to_string()
}

fn assert_customer_page(path: &str, html: &str, headers: &reqwest::header::HeaderMap, frame: bool) {
    assert!(!html.to_ascii_lowercase().contains("<script"), "{path}: a customer page has no script");
    assert!(!html.contains("<style"), "{path}: no inline style block");
    let csp = headers["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("default-src 'none'") && csp.contains("style-src 'self'") && !csp.contains("script-src"), "{path}: {csp}");
    let xfo = headers["x-frame-options"].to_str().unwrap();
    if frame {
        assert!(csp.contains("frame-ancestors 'self'") && xfo == "SAMEORIGIN", "{path}: {csp} {xfo}");
    } else {
        assert!(csp.contains("frame-ancestors 'none'") && xfo == "DENY", "{path}: {csp} {xfo}");
    }
    assert_eq!(headers["cache-control"], "no-store");
}

#[tokio::test]
async fn customer_pages_have_no_script_and_a_strict_csp() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let (_, inv) =
        t.post("/v1/invoice", &key, &json!({ "price_amount": 25, "price_currency": "eur", "order_description": "Two mugs" })).await;
    let btc = create_payment(&t, &key, json!({ "order_id": "A-1" })).await;
    let usdt = create_payment(&t, &key, json!({ "pay_currency": "usdttrc20" })).await;
    let usdc = create_payment(&t, &key, json!({ "pay_currency": "usdc" })).await;
    let sol = create_payment(&t, &key, json!({ "pay_currency": "usdcsol" })).await;

    let (status, html, h) = page(&t, &path_of(&inv["invoice_url"])).await;
    assert_eq!(status, 200);
    assert_customer_page("invoice", &html, &h, false);
    assert!(html.contains("25.00 EUR") && html.contains("Two mugs"));
    assert!(html.contains("value=\"usdttrc20\"") && html.contains("class=\"glyph\""), "coins with marks");

    let btc_path = path_of(&btc["payment_url"]);
    let (status, html, h) = page(&t, &btc_path).await;
    assert_eq!(status, 200);
    assert_customer_page("payment", &html, &h, false);
    assert!(html.contains(&format!("bitcoin:{}?amount=0.001", btc["pay_address"].as_str().unwrap())), "BIP21 wallet link");
    assert!(html.contains("user-select") || html.contains("class=\"sel"), "copy-friendly values");
    assert!(html.contains(&format!("src=\"{btc_path}/status\"")), "status frame");
    assert!(html.contains("Send BTC on Bitcoin Signet only."));

    let (_, html, _) = page(&t, &path_of(&usdt["payment_url"])).await;
    assert!(html.contains("Send USDT on Tron Nile (TRC-20) only."), "token network warning");
    assert!(html.contains("tron:") && html.contains(";token=T"), "TRC-20 wallet link");
    let (_, html, _) = page(&t, &path_of(&usdc["payment_url"])).await;
    assert!(html.contains("ethereum:0x") && html.contains("@11155111/transfer?address="), "EIP-681 token transfer");
    let (_, html, _) = page(&t, &path_of(&sol["payment_url"])).await;
    assert!(html.contains(";spl-token="), "Solana Pay with the mint");

    let (status, html, h) = page(&t, &format!("{btc_path}/status")).await;
    assert_eq!(status, 200);
    assert_customer_page("status frame", &html, &h, true);
    assert!(html.contains("http-equiv=\"refresh\" content=\"4;url=") && html.contains("seen=waiting"));
    assert!(html.contains("animation-duration:") && html.contains("cd-x"), "countdown with the time left as its duration");

    let (status, html, _) = page(&t, "/pay/doesnotexist").await;
    assert_eq!(status, 404);
    assert!(html.contains("does not exist") && !html.contains("<script"));
    let (_, css, h) = page(&t, "/assets/checkout.css").await;
    assert_eq!(h["content-type"], "text/css; charset=utf-8");
    assert!(css.contains("prefers-reduced-motion") && css.contains("@property --sec"));
}

/// More confirmations for the first deposit of a payment.
async fn reconfirm(t: &common::TestApp, id: i64, confirmations: i32) {
    let p = t.app.store.payment_any(id).await.unwrap().unwrap();
    let net = t.app.store.network(p.network_id).await.unwrap();
    let drivers = t.app.drivers();
    let driver = drivers.get(&net.chain).unwrap();
    let mock = turmite::chains::driver::as_mock(&driver).unwrap();
    let key = format!("tx-{id}-0");
    let obs = turmite::chains::driver::Observation {
        chain_tx_key: key.clone(),
        txid: key,
        vout: None,
        amount: p.pay_amount,
        confirmations,
        block_height: None,
    };
    mock.inject(&p.pay_address, obs).await;
    turmite::worker::process_payment(&t.app, id).await.unwrap();
}

#[tokio::test]
async fn status_frame_follows_the_payment() {
    let t = common::start(Default::default()).await;
    let (_, key) = t.project().await;
    let btc_net = t.app.store.network_by_chain("bitcoin").await.unwrap().unwrap();
    t.app.store.set_network_confirmations(btc_net.network_id, 3).await.unwrap();
    let p = create_payment(&t, &key, json!({ "success_url": "https://shop.example.com/thanks" })).await;
    let id = common::pid(&p);
    let frame = format!("{}/status", path_of(&p["payment_url"]));

    let (_, html, _) = page(&t, &frame).await;
    assert!(html.contains("st-waiting fresh") && html.contains("Waiting for payment"));
    let (_, html, _) = page(&t, &format!("{frame}?seen=waiting")).await;
    assert!(html.contains("class=\"st st-waiting\""), "a state already shown does not replay its entrance");
    assert!(html.contains("--phase:-"), "looping animations take their phase from the clock");

    t.deposit(id, "0.001", 1).await;
    let (_, html, _) = page(&t, &format!("{frame}?seen=waiting")).await;
    assert!(html.contains("st-confirming fresh") && html.contains("1 of 3") && html.contains("seen=confirming-1"));
    assert_eq!(html.matches("class=\"on n\"").count(), 1, "the new segment animates in");
    let (_, html, _) = page(&t, &format!("{frame}?seen=confirming-1")).await;
    assert!(!html.contains("on n") && !html.contains(" fresh"));

    reconfirm(&t, id, 3).await;
    let (_, html, _) = page(&t, &format!("{frame}?seen=confirming-2")).await;
    assert!(html.contains("st-paid fresh") && html.contains("Return to Test shop"));
    assert!(html.contains("href=\"https://shop.example.com/thanks\" target=\"_top\""));
    assert!(!html.contains("http-equiv=\"refresh\""), "a final state stops refreshing");
    let done = format!("<a class=\"btn btn-sm btn-2\" href=\"{}\" target=\"_top\">Done</a>", path_of(&p["payment_url"]));
    assert!(html.contains(&done), "the open page gets a link to the result: {html}");
    let (_, html, _) = page(&t, &frame).await;
    assert!(!html.contains(">Done</a>"), "a page loaded after the end shows the result already");
    let (_, html, _) = page(&t, &path_of(&p["payment_url"])).await;
    assert!(html.contains("Amount paid") && !html.contains("Open in wallet"), "a paid page shows the result only");

    // Expired and partly paid.
    let e = create_payment(&t, &key, json!({})).await;
    let eid = common::pid(&e);
    sqlx::query(
        "UPDATE payments SET created_at = now() - interval '2 hours', expires_at = now() - interval '1 minute' WHERE payment_id = $1",
    )
    .bind(eid)
    .execute(&t.app.store.pool)
    .await
    .unwrap();
    t.tick().await;
    let (_, html, _) = page(&t, &format!("{}/status", path_of(&e["payment_url"]))).await;
    assert!(html.contains("st-expired") && html.contains("Do not send funds"));
    let part = create_payment(&t, &key, json!({})).await;
    t.deposit(common::pid(&part), "0.0004", 3).await;
    let (_, html, _) = page(&t, &format!("{}/status", path_of(&part["payment_url"]))).await;
    assert!(html.contains("st-partial") && html.contains("Send 0.0006 BTC more"), "{html}");
}

#[tokio::test]
async fn branding_is_applied() {
    let t = common::start(Default::default()).await;
    let (pid, key) = t.project().await;
    let cookie = admin_cookie(&t).await;
    let base = format!("/dashboard/p/{pid}/settings/checkout");
    let r = dash_post(
        &t,
        &cookie,
        &base,
        &[
            ("accent", "#1D4ED8"),
            ("theme", "dark"),
            ("message", "Orders ship on Monday."),
            ("support_email", "help@shop.example.com"),
            ("support_url", "https://shop.example.com/help"),
            ("return_url", ""),
            ("return_hosts", ""),
        ],
    )
    .await;
    assert_eq!(r.status(), 303);
    assert!(flash(&r).starts_with("ok"), "{}", flash(&r));
    let p = create_payment(&t, &key, json!({})).await;
    let (_, html, _) = page(&t, &path_of(&p["payment_url"])).await;
    assert!(html.contains("data-theme=\"dark\"") && html.contains("--accent-l:#1d4ed8;--accent-l-fg:#ffffff"));
    assert!(html.contains("Orders ship on Monday.") && html.contains("mailto:help@shop.example.com"));
    assert!(html.contains("Test shop"));
    let (_, html, _) = page(&t, &format!("{}/status", path_of(&p["payment_url"]))).await;
    assert!(html.contains("data-theme=\"dark\""), "the frame has the same theme");

    // Bad values are refused.
    let r = dash_post(&t, &cookie, &base, &[("accent", "blue"), ("theme", "auto")]).await;
    assert!(flash(&r).starts_with("error"));
    let long = "x".repeat(281);
    let r = dash_post(&t, &cookie, &base, &[("accent_default", "1"), ("theme", "auto"), ("message", &long)]).await;
    assert!(flash(&r).starts_with("error"));

    // The settings page and its preview.
    let r = t.client.get(t.url(&base)).header("cookie", &cookie).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let html = r.text().await.unwrap();
    assert!(html.contains("name=\"preview\"") && html.contains("settings/preview?state=waiting"));
    for state in ["waiting", "confirming", "paid", "expired"] {
        let r = t
            .client
            .get(t.url(&format!("/dashboard/p/{pid}/settings/preview?state={state}")))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        assert!(r.headers()["content-security-policy"].to_str().unwrap().contains("frame-ancestors 'self'"));
        assert_eq!(r.headers()["x-frame-options"], "SAMEORIGIN");
        let html = r.text().await.unwrap();
        assert!(!html.contains("<script") && html.contains("data-theme=\"dark\""));
        let r = t
            .client
            .get(t.url(&format!("/dashboard/p/{pid}/settings/preview/status?state={state}")))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
    }
}

#[tokio::test]
async fn return_url_rules() {
    let t = common::start(Default::default()).await;
    let (pid, key) = t.project().await;
    let cookie = admin_cookie(&t).await;
    let base = format!("/dashboard/p/{pid}/settings/checkout");
    let save = |ret: &'static str, hosts: &'static str| {
        let (t, cookie, base) = (&t, cookie.clone(), base.clone());
        async move {
            flash(
                &dash_post(t, &cookie, &base, &[("accent_default", "1"), ("theme", "auto"), ("return_url", ret), ("return_hosts", hosts)])
                    .await,
            )
        }
    };
    assert!(
        save("https://evil.example.org/", "shop.example.com").await.starts_with("error"),
        "project return URL must use an allowed host"
    );
    assert!(save("https://shop.example.com/orders", "shop.example.com\nwww.shop.example.com").await.starts_with("ok"));

    let bad = t
        .post(
            "/v1/payment",
            &key,
            &json!({ "price_amount": 1, "price_currency": "usd", "pay_currency": "btc", "success_url": "https://evil.example.org/x" }),
        )
        .await;
    assert_eq!(bad.0, 400, "{}", bad.1);
    let bad = t
        .post("/v1/invoice", &key, &json!({ "price_amount": 1, "price_currency": "usd", "cancel_url": "https://evil.example.org/x" }))
        .await;
    assert_eq!(bad.0, 400, "{}", bad.1);
    let ok = create_payment(&t, &key, json!({ "cancel_url": "https://www.shop.example.com/cart" })).await;

    // Each payment keeps the return URL of its creation time; success_url wins.
    let first = create_payment(&t, &key, json!({})).await;
    assert!(save("https://shop.example.com/account", "shop.example.com").await.starts_with("ok"));
    let second = create_payment(&t, &key, json!({})).await;
    let third = create_payment(&t, &key, json!({ "success_url": "https://shop.example.com/thanks" })).await;
    for (p, want) in [
        (&first, "https://shop.example.com/orders"),
        (&second, "https://shop.example.com/account"),
        (&third, "https://shop.example.com/thanks"),
    ] {
        let id = common::pid(p);
        t.deposit(id, "0.001", 5).await;
        let (_, html, _) = page(&t, &format!("{}/status", path_of(&p["payment_url"]))).await;
        assert!(html.contains(&format!("href=\"{want}\" target=\"_top\"")), "{want}");
    }
    let _ = ok;
}

#[tokio::test]
async fn checkout_domain_serves_only_customer_pages() {
    let t = common::start(Default::default()).await;
    let (pid, key) = t.project().await;
    let (other_pid, other_key) = t.project().await;
    let cookie = admin_cookie(&t).await;

    // Server admin with a two-factor code (off on testnet) sets the domain.
    let r =
        dash_post(&t, &cookie, &format!("/dashboard/p/{pid}/settings/domain"), &[("domain", "Pay.Shop.Example.com"), ("totp", "")]).await;
    assert!(flash(&r).starts_with("ok"), "{}", flash(&r));
    let r =
        dash_post(&t, &cookie, &format!("/dashboard/p/{other_pid}/settings/domain"), &[("domain", "pay.shop.example.com"), ("totp", "")])
            .await;
    assert!(flash(&r).starts_with("error"), "a domain belongs to one project");
    let r = dash_post(&t, &cookie, &format!("/dashboard/p/{other_pid}/settings/domain"), &[("domain", "127.0.0.1"), ("totp", "")]).await;
    assert!(flash(&r).starts_with("error"), "not an IP or the site host");

    let p = create_payment(&t, &key, json!({})).await;
    let url = p["payment_url"].as_str().unwrap();
    assert!(url.starts_with("https://pay.shop.example.com/pay/"), "{url}");
    let (_, inv) = t.post("/v1/invoice", &key, &json!({ "price_amount": 5, "price_currency": "usd" })).await;
    assert!(inv["invoice_url"].as_str().unwrap().starts_with("https://pay.shop.example.com/i/"));
    let other = create_payment(&t, &other_key, json!({})).await;
    assert!(other["payment_url"].as_str().unwrap().starts_with(&t.base), "other projects keep the site URL");

    let on_domain = |path: String| {
        let t = &t;
        async move { t.client.get(t.url(&path)).header("host", "pay.shop.example.com").send().await.unwrap().status().as_u16() }
    };
    let pay = path_of(&p["payment_url"]);
    assert_eq!(on_domain(pay.clone()).await, 200);
    assert_eq!(on_domain(format!("{pay}/status")).await, 200);
    assert_eq!(on_domain(path_of(&inv["invoice_url"])).await, 200);
    assert_eq!(on_domain("/v1/status".into()).await, 200);
    assert_eq!(on_domain("/assets/checkout.css".into()).await, 200);
    assert_eq!(on_domain("/healthz".into()).await, 200);
    assert_eq!(on_domain("/favicon.svg".into()).await, 200);
    assert_eq!(on_domain("/dashboard/login".into()).await, 404, "never the dashboard");
    assert_eq!(on_domain("/".into()).await, 404);
    assert_eq!(on_domain("/dashboard".into()).await, 404);
    assert_eq!(on_domain(path_of(&other["payment_url"])).await, 404, "another project's payment");
    // The site host still serves the dashboard and every project.
    assert_eq!(t.client.get(t.url("/dashboard/login")).send().await.unwrap().status(), 200);
    assert_eq!(t.client.get(t.url(&path_of(&other["payment_url"]))).send().await.unwrap().status(), 200);

    // Only server admins change it.
    let member = t.app.store.create_user("dev@shop.example.com", "unused", "member").await.unwrap();
    t.app.store.set_member(pid, member, "project_admin").await.unwrap();
    let token = "member-session-token";
    t.app
        .store
        .create_session(&turmite::crypto::sha256_hex(token.as_bytes()), member, None, None, chrono::Utc::now() + chrono::Duration::hours(1))
        .await
        .unwrap();
    let r =
        dash_post(&t, &format!("turmite_session={token}"), &format!("/dashboard/p/{pid}/settings/domain"), &[("domain", ""), ("totp", "")])
            .await;
    assert_ne!(r.status(), 303, "a project admin cannot change the domain");
    assert_eq!(t.app.store.checkout_domain(pid).await.unwrap().as_deref(), Some("pay.shop.example.com"));
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, 120, 255]));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img).write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

async fn upload(t: &common::TestApp, cookie: &str, pid: i64, name: &str, bytes: &[u8]) -> Response {
    let boundary = "----turmiteboundary7";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"logo\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(
        format!("\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"logo_url\"\r\n\r\n\r\n--{boundary}--\r\n").as_bytes(),
    );
    t.client
        .post(t.url(&format!("/dashboard/p/{pid}/settings/checkout/logo")))
        .header("cookie", cookie)
        .header("origin", &t.base)
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .body(body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn logo_upload_is_checked_scaled_and_served() {
    let t = common::start(Default::default()).await;
    let (pid, key) = t.project().await;
    let cookie = admin_cookie(&t).await;

    let r = upload(&t, &cookie, pid, "logo.png", &png(1024, 600)).await;
    assert_eq!(r.status(), 303);
    assert!(flash(&r).starts_with("ok"), "{}", flash(&r));
    let b = t.app.store.branding(pid).await.unwrap();
    let hash = b.logo_hash.clone().expect("logo stored");
    let r = t.client.get(t.url(&format!("/branding/{pid}/logo-{hash}.png"))).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "image/png");
    assert_eq!(r.headers()["cache-control"], "public, max-age=31536000, immutable");
    assert_eq!(r.headers()["content-security-policy"], "default-src 'none'; img-src 'self'");
    let img = image::load_from_memory(&r.bytes().await.unwrap()).unwrap();
    assert_eq!((img.width(), img.height()), (512, 300), "scaled to fit 512 x 512");
    assert_eq!(t.client.get(t.url(&format!("/branding/{pid}/logo-0000000000000000.png"))).send().await.unwrap().status(), 404);
    let p = create_payment(&t, &key, json!({})).await;
    let (_, html, _) = page(&t, &path_of(&p["payment_url"])).await;
    assert!(html.contains(&format!("/branding/{pid}/logo-{hash}.png")));

    // Refused: SVG, a huge pixel size, a file over 2 MB, something that is not an image.
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1"><script>alert(1)</script></svg>"#;
    for (name, bytes, want) in [
        ("logo.svg", svg.to_vec(), "SVG"),
        ("wide.png", png(9000, 4), "pixels"),
        ("big.png", vec![7u8; 2 * 1024 * 1024 + 300_000], "upload failed"),
        ("logo.gif", b"GIF89a\x01\x00\x01\x00\x00\x00\x00;".to_vec(), "PNG, JPEG or WebP"),
    ] {
        let r = upload(&t, &cookie, pid, name, &bytes).await;
        let msg = flash(&r);
        assert!(msg.starts_with("error") && msg.contains(want), "{name}: {msg}");
    }
    assert_eq!(t.app.store.branding(pid).await.unwrap().logo_hash.as_deref(), Some(hash.as_str()), "the old logo stays");

    let r = t
        .client
        .post(t.url(&format!("/dashboard/p/{pid}/settings/checkout/logo")))
        .header("cookie", &cookie)
        .header("origin", &t.base)
        .header("content-type", "multipart/form-data; boundary=b")
        .body("--b\r\nContent-Disposition: form-data; name=\"remove\"\r\n\r\n1\r\n--b--\r\n")
        .send()
        .await
        .unwrap();
    assert!(flash(&r).starts_with("ok"));
    assert!(t.app.store.branding(pid).await.unwrap().logo_hash.is_none());
}

#[test]
fn logo_processing_without_a_server() {
    let (out, hash) = turmite::branding::process_logo(&png(100, 50)).unwrap();
    assert_eq!(hash.len(), 16);
    let img = image::load_from_memory(&out).unwrap();
    assert_eq!((img.width(), img.height()), (100, 50), "small logos keep their size");
    let mut jpeg = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::new(2000, 1000))
        .write_to(&mut Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
        .unwrap();
    let (out, _) = turmite::branding::process_logo(&jpeg).unwrap();
    assert_eq!(&out[..8], b"\x89PNG\r\n\x1a\n", "re-encoded as PNG");
    assert!(turmite::branding::process_logo(b"not an image").is_err());
}
