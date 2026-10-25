//! Admin pages of the ops stream: Networks (node health, assets) and Updates.

mod common;

use chrono::{Duration, Utc};
use serde_json::json;

/// A root account with a session. Returns the cookie header value.
async fn admin(t: &common::TestApp) -> String {
    let store = &t.app.store;
    let hash = turmite::auth::hash_password("correct horse battery staple").await.unwrap();
    let uid = store.create_user("root@example.com", &hash, "root").await.unwrap();
    let token = "ops-test-session-token";
    store.create_session(&turmite::crypto::sha256_hex(token.as_bytes()), uid, None, None, Utc::now() + Duration::hours(1)).await.unwrap();
    format!("turmite_session={token}")
}

async fn page(t: &common::TestApp, cookie: &str, path: &str) -> (u16, String) {
    let r = t.client.get(t.url(path)).header("cookie", cookie).send().await.unwrap();
    (r.status().as_u16(), r.text().await.unwrap())
}

/// Post a form like a browser on the site. Returns the status and the flash text.
async fn post(t: &common::TestApp, cookie: &str, path: &str, body: &str) -> (u16, String) {
    let r = t
        .client
        .post(t.url(path))
        .header("cookie", cookie)
        .header("origin", &t.base)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    let flash = r
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|v| v.strip_prefix("turmite_flash="))
        .map(|v| url::form_urlencoded::parse(format!("x={}", v.split(';').next().unwrap_or("")).as_bytes()).next().unwrap().1.into_owned())
        .unwrap_or_default();
    (r.status().as_u16(), flash)
}

#[tokio::test]
async fn networks_page_and_assets() {
    let t = common::start(common::TestOptions::default()).await;
    let cookie = admin(&t).await;
    let (status, html) = page(&t, &cookie, "/dashboard/admin/networks").await;
    assert_eq!(status, 200);
    assert!(html.contains("Assets ("), "token section");

    let base = t.app.store.list_networks().await.unwrap().into_iter().find(|n| n.chain == "base").unwrap();
    let contract = format!("0x{}", "ab".repeat(20));
    let path = format!("/dashboard/admin/networks/{}/tokens/add", base.network_id);
    let (status, flash) = post(&t, &cookie, &path, &format!("symbol=USDT&ticker=usdtbase&decimals=6&contract={contract}&totp=")).await;
    assert_eq!(status, 303);
    assert!(flash.contains("usdtbase added"), "{flash}");
    let ans = t.app.store.list_asset_networks().await.unwrap();
    let added = ans.iter().find(|a| a.ticker == "usdtbase").expect("token row");
    assert_eq!(added.contract_address.as_deref(), Some(contract.as_str()));

    // The same ticker again, a bad contract and a chain without tokens are refused.
    let (_, flash) = post(&t, &cookie, &path, &format!("symbol=USDC&ticker=usdtbase&decimals=6&contract={contract}")).await;
    assert!(flash.contains("in use"), "{flash}");
    let (_, flash) = post(&t, &cookie, &path, "symbol=DAI&ticker=daibase&decimals=18&contract=0x1234").await;
    assert!(flash.contains("not a"), "{flash}");
    let btc = t.app.store.list_networks().await.unwrap().into_iter().find(|n| n.chain == "bitcoin").unwrap();
    let (_, flash) = post(
        &t,
        &cookie,
        &format!("/dashboard/admin/networks/{}/tokens/add", btc.network_id),
        "symbol=USDT&ticker=usdtbtc&decimals=6&contract=x",
    )
    .await;
    assert!(flash.contains("no token contracts"), "{flash}");

    // Turn every Base asset off except usdtbase.
    let path = format!("/dashboard/admin/networks/{}/tokens", base.network_id);
    let (status, _) = post(&t, &cookie, &path, &format!("t{}=1&totp=", added.asset_network_id)).await;
    assert_eq!(status, 303);
    let ans = t.app.store.list_asset_networks().await.unwrap();
    for a in ans.iter().filter(|a| a.network.network_id == base.network_id) {
        assert_eq!(a.is_enabled, a.ticker == "usdtbase", "{}", a.ticker);
    }
    let audit = t.app.store.list_audit(None, turmite::db::Page::new(50, 0)).await.unwrap();
    assert!(audit.iter().any(|r| r.action == "token.add"));
    assert!(audit.iter().any(|r| r.action == "token.toggle"));
}

/// Log lines kept in memory.
#[derive(Clone, Default)]
struct Logs(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Logs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn worker_without_hot_identity_warns_at_start() {
    let logs = Logs::default();
    let sink = logs.clone();
    let subscriber = tracing_subscriber::fmt().with_ansi(false).with_writer(move || sink.clone()).finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    common::start(common::TestOptions { with_hot_identity: false, ..Default::default() }).await;
    let text = String::from_utf8_lossy(&logs.0.lock().unwrap()).to_string();
    assert!(text.contains(turmite::app::NO_HOT_IDENTITY), "{text}");

    logs.0.lock().unwrap().clear();
    common::start(common::TestOptions::default()).await;
    assert!(!String::from_utf8_lossy(&logs.0.lock().unwrap()).contains(turmite::app::NO_HOT_IDENTITY));
}

#[tokio::test]
async fn counters_skip_indexes_after_a_restore() {
    let t = common::start(Default::default()).await;
    let store = &t.app.store;
    let (pid, _) = t.project().await;
    let tron = store.network_by_chain("tron").await.unwrap().unwrap();
    let btc = store.network_by_chain("bitcoin").await.unwrap().unwrap();
    assert_eq!(store.next_network_index(tron.network_id).await.unwrap(), 1);
    assert_eq!(store.next_family_index("evm").await.unwrap(), 1);
    assert_eq!(store.next_wallet_index(pid, btc.network_id).await.unwrap(), 0);
    let free = store.insert_deposit_address(tron.network_id, None, 7, "TFreePoolAddress", None).await.unwrap();
    let taken = store.insert_deposit_address(tron.network_id, Some(pid), 8, "TTakenAddress", None).await.unwrap();

    assert_eq!(store.raise_address_counters(500).await.unwrap(), 1);
    assert_eq!(store.next_network_index(tron.network_id).await.unwrap(), 502);
    assert_eq!(store.next_family_index("evm").await.unwrap(), 502);
    assert_eq!(store.next_wallet_index(pid, btc.network_id).await.unwrap(), 501);
    let ids: Vec<i64> = sqlx::query_scalar("SELECT deposit_address_id FROM deposit_addresses").fetch_all(&store.pool).await.unwrap();
    assert!(!ids.contains(&free) && ids.contains(&taken), "only the free pool address is deleted");
}

#[tokio::test]
async fn last_block_moves_only_forward() {
    let t = common::start(common::TestOptions::default()).await;
    let store = &t.app.store;
    let n = store.list_networks().await.unwrap().remove(0);
    store.advance_last_block(n.network_id, 5000).await.unwrap();
    let first = store.network(n.network_id).await.unwrap().last_block_at;
    store.advance_last_block(n.network_id, 4999).await.unwrap();
    let net = store.network(n.network_id).await.unwrap();
    assert_eq!((net.last_block, net.last_block_at), (Some(5000), first), "a lower height from a slower node is ignored");
    store.advance_last_block(n.network_id, 5001).await.unwrap();
    assert_eq!(store.network(n.network_id).await.unwrap().last_block, Some(5001));
    // A reset test chain starts again from a low height.
    store.advance_last_block(n.network_id, 5).await.unwrap();
    assert_eq!(store.network(n.network_id).await.unwrap().last_block, Some(5));
    // Fixed (mock) drivers: the node check does nothing and does not fail.
    turmite::chains::registry::check_nodes(&t.app).await;
}

/// Real bitcoind (deploy/testnets regtest): a dead node of higher priority and a live one.
#[tokio::test]
async fn node_check_on_regtest() {
    if !common::chain_enabled("bitcoin") {
        return;
    }
    let t = common::start(common::TestOptions { drivers: None, ..Default::default() }).await;
    let app = &t.app;
    let btc = app.store.list_networks().await.unwrap().into_iter().find(|n| n.chain == "bitcoin").unwrap();
    let dead = "http://devuser:devpass@127.0.0.1:1";
    let live = "http://devuser:devpass@127.0.0.1:18443";
    let dead_id =
        app.store.add_node(btc.network_id, "dead", &app.sealer.seal_str("node_url", dead), "http://127.0.0.1:1/", 0).await.unwrap();
    let live_id =
        app.store.add_node(btc.network_id, "regtest", &app.sealer.seal_str("node_url", live), "http://127.0.0.1:18443/", 1).await.unwrap();

    turmite::chains::registry::check_nodes(app).await;
    assert!(app.drivers().get("bitcoin").is_some(), "the live node serves the network");
    assert_eq!(app.health.active(btc.network_id), Some(live_id));
    let nodes = app.store.list_nodes(Some(btc.network_id)).await.unwrap();
    let dead_row = nodes.iter().find(|n| n.node_id == dead_id).unwrap();
    let error = dead_row.last_error.clone().unwrap_or_default();
    assert!(!error.is_empty() && !error.contains("devpass"), "{error}");
    assert!(nodes.iter().find(|n| n.node_id == live_id).unwrap().last_ok_at.is_some());
    let active: std::collections::HashMap<String, i64> =
        app.store.setting(turmite::chains::registry::ACTIVE_NODES_KEY).await.unwrap().unwrap_or_default();
    assert_eq!(active.get(&btc.network_id.to_string()), Some(&live_id), "the worker publishes its node");

    // A second round keeps the live node.
    turmite::chains::registry::check_nodes(app).await;
    assert_eq!(app.health.active(btc.network_id), Some(live_id));
}

#[tokio::test]
async fn update_banners_and_settings() {
    let t = common::start(common::TestOptions::default()).await;
    let cookie = admin(&t).await;
    let (status, html) = page(&t, &cookie, "/dashboard/admin/updates").await;
    assert_eq!(status, 200);
    assert!(html.contains("Check for updates every day") && html.contains("install.sh"));
    assert!(!html.contains("banner-bad"));

    let state = json!({
        "checked_at": Utc::now(),
        "error": null,
        "latest": { "version": "99.0.0", "url": "https://github.com/loruorg/turmite/releases/tag/v99.0.0", "published_at": null },
        "advisories": [{ "id": "GHSA-test-test-test", "summary": "Test flaw", "severity": "critical",
                         "url": "https://github.com/loruorg/turmite/security/advisories/GHSA-test-test-test",
                         "ranges": ["< 99.0.0"], "patched": ["99.0.0"], "published_at": null }],
        "emailed": []
    });
    t.app.store.set_setting("update_state", &state).await.unwrap();
    let (_, html) = page(&t, &cookie, "/dashboard/admin/networks").await;
    assert!(html.contains("banner-bad") && html.contains("GHSA-test-test-test"), "advisory banner on every page");
    assert!(html.contains("Turmite 99.0.0 is available"));

    // Dismissing hides the release banner, not the advisory.
    let (status, _) = post(&t, &cookie, "/dashboard/admin/updates/dismiss", "").await;
    assert_eq!(status, 303);
    let (_, html) = page(&t, &cookie, "/dashboard/admin/updates").await;
    assert!(!html.contains("99.0.0 is available"));
    assert!(html.contains("banner-bad") && html.contains("Affected"));

    // Turned off: no banners.
    let (status, flash) = post(&t, &cookie, "/dashboard/admin/updates/settings", "totp=").await;
    assert_eq!(status, 303);
    assert!(flash.contains("off"), "{flash}");
    assert!(!turmite::updates::enabled(&t.app).await);
    let (_, html) = page(&t, &cookie, "/dashboard/admin/updates").await;
    assert!(!html.contains("banner-bad"));
}
