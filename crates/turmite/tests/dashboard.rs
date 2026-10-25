//! Dashboard sign-in, two-factor, permissions and form safety.

mod common;

use reqwest::{Client, header};

/// A browser-like client: keeps cookies, sends the site's Origin, does not follow redirects.
fn browser() -> Client {
    Client::builder().cookie_store(true).redirect(reqwest::redirect::Policy::none()).build().unwrap()
}

async fn form(c: &Client, t: &common::TestApp, path: &str, fields: &[(&str, &str)]) -> reqwest::Response {
    c.post(t.url(path)).header(header::ORIGIN, &t.base).form(fields).send().await.unwrap()
}

fn location(r: &reqwest::Response) -> String {
    r.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string()).unwrap_or_default()
}

async fn setup_root(t: &common::TestApp) -> Client {
    let c = browser();
    let r = c.get(t.url("/dashboard")).send().await.unwrap();
    assert_eq!(location(&r), "/dashboard/setup");
    let r = form(
        &c,
        t,
        "/dashboard/setup",
        &[("email", "root@example.com"), ("password", "a very long password"), ("password2", "a very long password")],
    )
    .await;
    assert_eq!(r.status(), 303);
    c
}

/// Turn on two-factor for the signed-in root. Returns the secret.
async fn enrol_totp(c: &Client, t: &common::TestApp) -> Vec<u8> {
    assert_eq!(form(c, t, "/dashboard/account/2fa/start", &[]).await.status(), 303);
    let user = t.app.store.user_by_email("root@example.com").await.unwrap().unwrap();
    let secret = t.app.sealer.open("totp", user.totp_secret.as_ref().unwrap()).unwrap().to_vec();
    let now = chrono::Utc::now().timestamp() as u64;
    form(c, t, "/dashboard/account/2fa/confirm", &[("code", &turmite::auth::totp::code(&secret, now))]).await;
    assert!(t.app.store.user(user.user_id).await.unwrap().totp_enabled);
    secret
}

/// A code that the server accepts once: forgets the last used step first.
async fn fresh_code(t: &common::TestApp, secret: &[u8]) -> String {
    sqlx::query("UPDATE users SET totp_last_step = 0").execute(&t.app.store.pool).await.unwrap();
    turmite::auth::totp::code(secret, chrono::Utc::now().timestamp() as u64)
}

#[tokio::test]
async fn first_run_creates_root_and_a_project() {
    let t = common::start(Default::default()).await;
    let c = setup_root(&t).await;
    let r = c.get(t.url("/dashboard")).send().await.unwrap();
    assert!(location(&r).starts_with("/dashboard/p/"), "{}", location(&r));
    let page = c.get(t.url(&location(&r))).send().await.unwrap().text().await.unwrap();
    assert!(page.contains("My shop"));
    // Setup is closed once a user exists.
    let r = browser().get(t.url("/dashboard/setup")).send().await.unwrap();
    assert_eq!(location(&r), "/dashboard/login");
}

#[tokio::test]
async fn login_with_two_factor() {
    let t = common::start(Default::default()).await;
    let c = setup_root(&t).await;
    // Enrol two-factor.
    assert_eq!(form(&c, &t, "/dashboard/account/2fa/start", &[]).await.status(), 303);
    let user = t.app.store.user_by_email("root@example.com").await.unwrap().unwrap();
    let secret = t.app.sealer.open("totp", user.totp_secret.as_ref().unwrap()).unwrap();
    let now = chrono::Utc::now().timestamp() as u64;
    let r = form(&c, &t, "/dashboard/account/2fa/confirm", &[("code", &turmite::auth::totp::code(&secret, now))]).await;
    assert_eq!(r.status(), 303);
    assert!(t.app.store.user(user.user_id).await.unwrap().totp_enabled);

    // A new browser: password, then the code. A code works once.
    let b = browser();
    let r = form(&b, &t, "/dashboard/login", &[("email", "root@example.com"), ("password", "a very long password")]).await;
    assert_eq!(location(&r), "/dashboard/login/code");
    let r = form(&b, &t, "/dashboard/login/code", &[("code", &turmite::auth::totp::code(&secret, now))]).await;
    assert_eq!(r.status(), 200, "the confirm step already used this code");
    let r = form(&b, &t, "/dashboard/login/code", &[("code", &turmite::auth::totp::code(&secret, now + 30))]).await;
    assert_eq!(location(&r), "/dashboard");
    let r = b.get(t.url("/dashboard/account")).send().await.unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn wrong_password_and_rate_limit() {
    let t = common::start(Default::default()).await;
    setup_root(&t).await;
    let b = browser();
    for _ in 0..10 {
        let r = form(&b, &t, "/dashboard/login", &[("email", "root@example.com"), ("password", "wrong password!!")]).await;
        assert!(r.text().await.unwrap().contains("Wrong email or password"));
    }
    let r = form(&b, &t, "/dashboard/login", &[("email", "root@example.com"), ("password", "a very long password")]).await;
    assert!(r.text().await.unwrap().contains("Too many attempts"));
}

#[tokio::test]
async fn cross_site_posts_are_refused() {
    let t = common::start(Default::default()).await;
    let c = setup_root(&t).await;
    let r = c
        .post(t.url("/dashboard/projects/new"))
        .header(header::ORIGIN, "https://evil.example")
        .form(&[("name", "Evil")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = c.post(t.url("/dashboard/projects/new")).form(&[("name", "No origin")]).send().await.unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(t.app.store.list_projects().await.unwrap().len(), 1);
}

#[tokio::test]
async fn members_see_only_their_projects_and_roles_apply() {
    let t = common::start(Default::default()).await;
    let root = setup_root(&t).await;
    let other = t.app.store.create_project("Other", &t.app.sealer.seal_str("ipn_secret", "x")).await.unwrap();
    let first = t.app.store.list_projects().await.unwrap()[0].project_id;
    let r = form(
        &root,
        &t,
        "/dashboard/admin/users",
        &[("email", "dev@example.com"), ("password", "another long password"), ("role", "member"), ("totp", "")],
    )
    .await;
    assert_eq!(r.status(), 303);
    let dev = t.app.store.user_by_email("dev@example.com").await.unwrap().unwrap();
    t.app.store.set_member(first, dev.user_id, "read_only").await.unwrap();

    let d = browser();
    form(&d, &t, "/dashboard/login", &[("email", "dev@example.com"), ("password", "another long password")]).await;
    assert_eq!(d.get(t.url(&format!("/dashboard/p/{first}"))).send().await.unwrap().status(), 200);
    assert_eq!(d.get(t.url(&format!("/dashboard/p/{other}"))).send().await.unwrap().status(), 404);
    assert_eq!(d.get(t.url("/dashboard/admin/users")).send().await.unwrap().status(), 403);
    // Read-only members cannot create API keys.
    let r = form(&d, &t, &format!("/dashboard/p/{first}/keys"), &[("label", "x"), ("totp", "")]).await;
    assert_eq!(r.status(), 403);
    assert!(t.app.store.list_api_keys(first).await.unwrap().is_empty());
}

#[tokio::test]
async fn admin_grants_and_node_removal_need_a_fresh_code() {
    let t = common::start(Default::default()).await;
    let root = setup_root(&t).await;
    let secret = enrol_totp(&root, &t).await;
    let pid = t.app.store.list_projects().await.unwrap()[0].project_id;
    let uid = t.app.store.create_user("ops@example.com", "x", "member").await.unwrap();

    // Make an admin.
    let path = format!("/dashboard/admin/users/{uid}/role");
    assert_eq!(form(&root, &t, &path, &[("role", "admin"), ("totp", "")]).await.status(), 303);
    assert_eq!(t.app.store.user(uid).await.unwrap().role, "member");
    let code = fresh_code(&t, &secret).await;
    form(&root, &t, &path, &[("role", "admin"), ("totp", &code)]).await;
    assert_eq!(t.app.store.user(uid).await.unwrap().role, "admin");
    // Back to member needs no code.
    form(&root, &t, &path, &[("role", "member"), ("totp", "")]).await;
    assert_eq!(t.app.store.user(uid).await.unwrap().role, "member");

    // Grant project admin. Developer needs no code.
    let path = format!("/dashboard/p/{pid}/team");
    form(&root, &t, &path, &[("email", "ops@example.com"), ("role", "project_admin"), ("totp", "")]).await;
    assert!(t.app.store.list_members(pid).await.unwrap().is_empty());
    form(&root, &t, &path, &[("email", "ops@example.com"), ("role", "developer"), ("totp", "")]).await;
    assert_eq!(t.app.store.list_members(pid).await.unwrap()[0].role, "developer");
    let code = fresh_code(&t, &secret).await;
    form(&root, &t, &path, &[("email", "ops@example.com"), ("role", "project_admin"), ("totp", &code)]).await;
    assert_eq!(t.app.store.list_members(pid).await.unwrap()[0].role, "project_admin");

    // Remove a node.
    let nid = t.app.store.list_networks().await.unwrap()[0].network_id;
    let node =
        t.app.store.add_node(nid, "n", &t.app.sealer.seal_str("node_url", "http://127.0.0.1:1"), "http://127.0.0.1:1", 0).await.unwrap();
    let path = format!("/dashboard/admin/nodes/{node}/delete");
    form(&root, &t, &path, &[("totp", "")]).await;
    assert_eq!(t.app.store.list_nodes(Some(nid)).await.unwrap().len(), 1);
    let code = fresh_code(&t, &secret).await;
    form(&root, &t, &path, &[("totp", &code)]).await;
    assert!(t.app.store.list_nodes(Some(nid)).await.unwrap().is_empty());
}

#[tokio::test]
async fn refund_is_recorded_only_after_funds_arrived() {
    let t = common::start(Default::default()).await;
    let root = setup_root(&t).await;
    let (pid, key) = t.project().await;
    let body = serde_json::json!({ "price_amount": 100, "price_currency": "usd", "pay_currency": "btc" });
    let id = common::pid(&t.post("/v1/payment", &key, &body).await.1);
    let path = format!("/dashboard/p/{pid}/payments/{id}/refunded");
    let cases = [
        ("waiting", "0", false),
        ("confirming", "0.001", false),
        ("sending", "0.001", false),
        ("expired", "0", false),
        ("expired", "0.001", true),
        ("partially_paid", "0.001", true),
        ("failed", "0.001", true),
        ("finished", "0.001", true),
    ];
    for (status, paid, allowed) in cases {
        sqlx::query("UPDATE payments SET status = $1, actually_paid = $2::numeric WHERE payment_id = $3")
            .bind(status)
            .bind(paid)
            .bind(id)
            .execute(&t.app.store.pool)
            .await
            .unwrap();
        let page = root.get(t.url(&format!("/dashboard/p/{pid}/payments/{id}"))).send().await.unwrap().text().await.unwrap();
        assert_eq!(page.contains("Mark as refunded"), allowed, "{status} {paid}: button");
        assert_eq!(form(&root, &t, &path, &[("totp", "")]).await.status(), 303);
        let now = t.app.store.payment(pid, id).await.unwrap().unwrap().status;
        assert_eq!(now.as_str() == "refunded", allowed, "{status} {paid}: refund recorded");
    }
}

/// The text of the flash cookie that a redirect sets.
fn flash(r: &reqwest::Response) -> String {
    r.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("turmite_flash="))
        .map(|v| url::form_urlencoded::parse(v.split(';').next().unwrap().as_bytes()).map(|(_, v)| v.into_owned()).collect())
        .unwrap_or_default()
}

/// A Solana driver whose node does not answer.
struct DeadSolanaNode;

#[async_trait::async_trait]
impl turmite::chains::PaymentDriver for DeadSolanaNode {
    fn chain(&self) -> &str {
        "solana"
    }
    async fn validate_address(&self, _address: &str) -> turmite::error::Result<bool> {
        Err(turmite::error::Error::Driver("solana getAccountInfo: connection refused".into()))
    }
    async fn derive_deposit(
        &self,
        _w: &turmite::chains::driver::WalletContext,
        _index: u64,
    ) -> turmite::error::Result<turmite::chains::driver::DerivedAddress> {
        Err(turmite::error::Error::Driver("no node".into()))
    }
    async fn observe(
        &self,
        _w: &turmite::chains::driver::WalletContext,
        _address: &str,
        _memo: Option<&str>,
        _asset: &turmite::chains::driver::AssetContext,
    ) -> turmite::error::Result<Vec<turmite::chains::driver::Observation>> {
        Ok(vec![])
    }
    async fn sweep(
        &self,
        _w: &turmite::chains::driver::WalletContext,
        _req: &turmite::chains::driver::SweepRequest,
        _asset: &turmite::chains::driver::AssetContext,
    ) -> turmite::error::Result<turmite::chains::driver::SweepResult> {
        Err(turmite::error::Error::Driver("no node".into()))
    }
}

#[tokio::test]
async fn wallet_save_tells_a_dead_node_from_a_bad_address() {
    let mut drivers = turmite::chains::DriverRegistry::all_mock();
    drivers.insert(std::sync::Arc::new(DeadSolanaNode));
    let t = common::start(common::TestOptions { drivers: Some(drivers), ..Default::default() }).await;
    let c = setup_root(&t).await;
    let pid = t.app.store.list_projects().await.unwrap()[0].project_id;
    let sol = t.app.store.network_by_chain("solana").await.unwrap().unwrap();
    let path = format!("/dashboard/p/{pid}/wallets/{}", sol.network_id);
    let r = form(&c, &t, &path, &[("address", "So11111111111111111111111111111111111111112"), ("totp", "")]).await;
    let text = flash(&r);
    assert!(text.contains("node is not available") && !text.contains("not a valid"), "{text}");
    assert!(t.app.store.project_wallet(pid, sol.network_id).await.unwrap().is_none());
}

#[tokio::test]
async fn api_key_is_shown_once_and_works() {
    let t = common::start(Default::default()).await;
    let c = setup_root(&t).await;
    let pid = t.app.store.list_projects().await.unwrap()[0].project_id;
    let html = form(&c, &t, &format!("/dashboard/p/{pid}/keys"), &[("label", "Shop"), ("totp", "")]).await.text().await.unwrap();
    let key = html.split("class=\"secret\"><code>").nth(1).unwrap().split("</code>").next().unwrap().to_string();
    assert!(key.starts_with("tm_"));
    let (status, _) = t.get("/v1/merchant/coins", &key).await;
    assert_eq!(status, 200);
    let again = c.get(t.url(&format!("/dashboard/p/{pid}/keys"))).send().await.unwrap().text().await.unwrap();
    assert!(!again.contains(&key));
}

#[tokio::test]
async fn sessions_end_on_logout() {
    let t = common::start(Default::default()).await;
    let c = setup_root(&t).await;
    assert_eq!(form(&c, &t, "/dashboard/logout", &[]).await.status(), 303);
    let r = c.get(t.url("/dashboard/account")).send().await.unwrap();
    assert_eq!(location(&r), "/dashboard/login");
}

#[tokio::test]
async fn password_change_ends_the_other_sessions() {
    let t = common::start(Default::default()).await;
    let a = setup_root(&t).await;
    let b = browser();
    let r = form(&b, &t, "/dashboard/login", &[("email", "root@example.com"), ("password", "a very long password")]).await;
    assert_eq!(location(&r), "/dashboard");
    let fields = [("current", "a very long password"), ("password", "another long password"), ("password2", "another long password")];
    assert_eq!(form(&a, &t, "/dashboard/account/password", &fields).await.status(), 303);
    assert_eq!(a.get(t.url("/dashboard/account")).send().await.unwrap().status(), 200, "this session stays");
    assert_eq!(location(&b.get(t.url("/dashboard/account")).send().await.unwrap()), "/dashboard/login");
}

#[tokio::test]
async fn dashboard_pages_render_without_errors() {
    let t = common::start(Default::default()).await;
    let c = setup_root(&t).await;
    let pid = t.app.store.list_projects().await.unwrap()[0].project_id;
    for path in ["", "/payments", "/invoices", "/wallets", "/keys", "/webhooks", "/settings", "/team"] {
        let r = c.get(t.url(&format!("/dashboard/p/{pid}{path}"))).send().await.unwrap();
        assert_eq!(r.status(), 200, "/p/{pid}{path}");
        let csp = r.headers()["content-security-policy"].to_str().unwrap().to_string();
        assert!(csp.contains("script-src 'self'") && csp.contains("frame-ancestors 'none'"));
    }
    for path in ["/dashboard/admin/networks", "/dashboard/admin/users", "/dashboard/admin/audit", "/dashboard/account"] {
        assert_eq!(c.get(t.url(path)).send().await.unwrap().status(), 200, "{path}");
    }
}
