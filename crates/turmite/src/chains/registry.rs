//! Builds the driver registry from the RPC nodes stored in the database, probes the
//! nodes and moves a network to its next node when the current one fails.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use crate::{
    app::App,
    chains::driver::{DriverRegistry, PaymentDriver},
    crypto::Sealer,
    db::{NodeRow, PgStore},
    domain::Network,
    error::{Error, Result},
};

/// Context `build` needs to connect drivers.
pub struct BuildContext<'a> {
    pub store: &'a PgStore,
    pub sealer: &'a Sealer,
    /// Install deposit seed. `None` in the API process: seed-derived chains are then
    /// built without signing keys where the driver allows it, or skipped.
    pub deposit_seed: Option<&'a str>,
}

/// Node URL of `node`, decrypted.
pub fn node_url(sealer: &Sealer, node: &NodeRow) -> Result<String> {
    sealer.open_str("node_url", &node.url_sealed).map(|s| s.to_string())
}

/// URL without credentials or query, for display and logs.
pub fn redact_url(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut u) => {
            let _ = u.set_username("");
            let _ = u.set_password(None);
            u.set_query(None);
            let path = u.path().to_string();
            // Providers often put the API key in the path.
            if path.len() > 12 {
                u.set_path("/redacted");
            }
            u.to_string()
        }
        Err(_) => "(invalid url)".into(),
    }
}

/// Connect one driver per enabled network that has an enabled node.
pub async fn build(ctx: &BuildContext<'_>) -> Result<DriverRegistry> {
    build_bound(ctx, &Health::default()).await
}

/// Like [`build`], and note in `health` which node serves each network.
pub async fn build_bound(ctx: &BuildContext<'_>, health: &Health) -> Result<DriverRegistry> {
    let mut reg = DriverRegistry::new();
    let mut active = HashMap::new();
    let networks = ctx.store.list_networks().await?;
    let nodes = ctx.store.list_nodes(None).await?;
    for network in networks.iter().filter(|n| n.is_enabled) {
        let urls: Vec<(i64, String)> = nodes
            .iter()
            .filter(|n| n.network_id == network.network_id && n.is_enabled)
            .filter_map(|n| match node_url(ctx.sealer, n) {
                Ok(url) => Some((n.node_id, url)),
                Err(e) => {
                    tracing::error!(node = n.node_id, error = %e, "cannot read node url");
                    None
                }
            })
            .collect();
        let mut connected = false;
        let mut seedless = false;
        for (node_id, url) in &urls {
            match connect(network, url, ctx.deposit_seed, ctx.store).await {
                Ok(driver) => {
                    tracing::info!(network = %network.code, node = node_id, "driver connected");
                    let _ = ctx.store.record_node_ok(*node_id, None).await;
                    reg.insert(driver);
                    active.insert(network.network_id, *node_id);
                    connected = true;
                    break;
                }
                // The API process builds seed-derived chains only when the driver allows it.
                Err(e) if ctx.deposit_seed.is_none() && e.to_string().contains(NO_SEED) => {
                    seedless = true;
                    break;
                }
                Err(e) => {
                    let msg = redact_error(&e.to_string());
                    tracing::warn!(network = %network.code, node = node_id, error = %msg, "node failed; trying the next one");
                    let _ = ctx.store.record_node_error(*node_id, &msg).await;
                }
            }
        }
        if !connected && !seedless && !urls.is_empty() {
            tracing::error!(network = %network.code, "no node of this network answered");
        }
    }
    let mut st = health.lock();
    st.active = active;
    st.standby.clear();
    drop(st);
    Ok(reg)
}

#[allow(dead_code)]
fn seed<'a>(network: &Network, seed: Option<&'a str>) -> Result<&'a str> {
    seed.ok_or_else(|| Error::Driver(format!("{}: {NO_SEED}", network.code)))
}

#[allow(unused_variables)]
async fn connect(network: &Network, url: &str, deposit_seed: Option<&str>, store: &PgStore) -> Result<Arc<dyn PaymentDriver>> {
    #[allow(unused_variables)]
    let confirmations = network.confirmations_required;
    Ok(match network.chain.as_str() {
        "bitcoin" => {
            let (url, user, pass) = split_credentials(url);
            Arc::new(crate::chains::btc::BtcDriver::connect(&url, user, pass).await?)
        }
        "monero" => Arc::new(crate::chains::xmr::XmrDriver::connect(url).await?),
        #[cfg(feature = "evm")]
        "ethereum" | "base" | "polygon" | "bsc" => Arc::new(crate::chains::evm::EvmDriver::connect(network, url, store.clone()).await?),
        #[cfg(feature = "sol")]
        "solana" => Arc::new(crate::chains::sol::SolDriver::connect(url, deposit_seed, network.is_testnet, confirmations).await?),
        #[cfg(feature = "tron")]
        "tron" => Arc::new(crate::chains::tron::TronDriver::connect(url, deposit_seed, network.is_testnet, confirmations).await?),
        #[cfg(feature = "ton")]
        "ton" => Arc::new(crate::chains::ton::TonDriver::connect(url, deposit_seed, network.is_testnet, confirmations).await?),
        other => return Err(Error::Driver(format!("this build has no driver for `{other}`"))),
    })
}

/// Move an `apikey` / `api_key` query parameter out of a URL, for APIs that take the
/// key in a header. Returns the URL without a trailing slash.
pub fn split_api_key(raw: &str) -> (String, Option<String>) {
    let Ok(mut u) = url::Url::parse(raw.trim()) else { return (raw.trim().trim_end_matches('/').to_string(), None) };
    let mut key = None;
    let rest: Vec<(String, String)> = u
        .query_pairs()
        .filter_map(|(k, v)| {
            if matches!(k.as_ref(), "apikey" | "api_key") {
                key = Some(v.into_owned());
                None
            } else {
                Some((k.into_owned(), v.into_owned()))
            }
        })
        .collect();
    if rest.is_empty() {
        u.set_query(None);
    } else {
        u.query_pairs_mut().clear().extend_pairs(rest);
    }
    (u.to_string().trim_end_matches('/').to_string(), key)
}

// ---- health and failover ---------------------------------------------------------

/// How often the worker probes every node. The API process probes at [`API_PROBE`].
pub const WORKER_PROBE: Duration = Duration::from_secs(30);
pub const API_PROBE: Duration = Duration::from_secs(120);
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// Failed probes in a row before a network moves to its next node.
const FAILOVER_AFTER: u32 = 2;
/// Good probes in a row before a network moves back to a node of higher priority.
const FAILBACK_AFTER: u32 = 2;
/// Setting: `{"<network_id>": <node_id>}`, the nodes that the worker uses.
pub const ACTIVE_NODES_KEY: &str = "active_nodes";
/// The error of `seed()`: the API process cannot build these drivers.
const NO_SEED: &str = "the deposit seed is only available in the worker";
/// The error of the default `PaymentDriver::tip_height`.
const NO_TIP: &str = "tip height is not available";

/// Which node serves each network in this process, and recent probe results.
#[derive(Default)]
pub struct Health {
    state: Mutex<HealthState>,
}

#[derive(Default)]
struct HealthState {
    /// network_id -> node_id whose driver is in the registry.
    active: HashMap<i64, i64>,
    /// node_id -> probe streaks.
    streaks: HashMap<i64, Streak>,
    /// Connected drivers of nodes that do not serve their network now.
    standby: HashMap<i64, Arc<dyn PaymentDriver>>,
    /// What this process last wrote to [`ACTIVE_NODES_KEY`].
    published: HashMap<String, i64>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Streak {
    ok: u32,
    failed: u32,
}

impl Health {
    fn lock(&self) -> MutexGuard<'_, HealthState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The node that serves `network_id` in this process.
    pub fn active(&self, network_id: i64) -> Option<i64> {
        self.lock().active.get(&network_id).copied()
    }

    /// True when the node connected in this process: it serves its network or waits as a standby.
    /// A connect checks that the node is on the network of the install's mode.
    pub fn connected(&self, network_id: i64, node_id: i64) -> bool {
        let st = self.lock();
        st.active.get(&network_id) == Some(&node_id) || st.standby.contains_key(&node_id)
    }
}

/// Blocks a node may trail the best node of its network before it counts as failed.
pub fn max_lag(chain: &str) -> i64 {
    match chain {
        "bitcoin" => 2,
        "monero" => 3,
        "ethereum" => 10,
        "base" | "polygon" => 60,
        "bsc" => 160,
        "solana" => 300,
        _ => 40,
    }
}

/// Time without a new block after which the dashboard warns.
pub fn stale_after(chain: &str) -> chrono::Duration {
    match chain {
        "bitcoin" => chrono::Duration::minutes(60),
        "monero" => chrono::Duration::minutes(20),
        _ => chrono::Duration::minutes(5),
    }
}

/// An error text without the credentials, query or key path of the URLs in it.
pub fn redact_error(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut rest = msg;
    while let Some(start) = ["https://", "http://", "wss://", "ws://"].iter().filter_map(|p| rest.find(p)).min() {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let end =
            tail.find(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '"' | '\'' | '<' | '>' | ',' | ';')).unwrap_or(tail.len());
        out.push_str(&redact_url(&tail[..end]));
        rest = &tail[end..];
    }
    out.push_str(rest);
    out.chars().take(500).collect()
}

enum Probe {
    /// Answered with its tip height.
    Height(i64),
    /// Connected, but the driver cannot tell its height.
    Up,
    Down(String),
    /// Not for this process (a seed-derived chain in the API process).
    Skip,
}

/// Probe the nodes every `every`, from a few seconds after start.
pub async fn health_loop(app: App, every: Duration) {
    tokio::time::sleep(Duration::from_secs(10)).await;
    let mut interval = tokio::time::interval(every);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        check_nodes(&app).await;
    }
}

/// Probe every enabled node once, record the results and move networks between nodes.
pub async fn check_nodes(app: &App) {
    if app.drivers_fixed() {
        return;
    }
    if let Err(e) = app.refresh_drivers().await {
        tracing::warn!(error = %e, "driver refresh failed");
    }
    let (networks, nodes) = match (app.store.list_networks().await, app.store.list_nodes(None).await) {
        (Ok(n), Ok(x)) => (n, x),
        (Err(e), _) | (_, Err(e)) => {
            tracing::warn!(error = %e, "node check: cannot read the networks");
            return;
        }
    };
    let mut set = tokio::task::JoinSet::new();
    for network in networks.into_iter().filter(|n| n.is_enabled) {
        let mine: Vec<NodeRow> = nodes.iter().filter(|n| n.network_id == network.network_id && n.is_enabled).cloned().collect();
        if mine.is_empty() {
            continue;
        }
        let app = app.clone();
        set.spawn(async move { check_network(&app, &network, &mine).await });
    }
    while set.join_next().await.is_some() {}
    if app.role.runs_worker() {
        publish_active(app).await;
    }
}

/// Save the worker's active nodes for the dashboard when they changed.
async fn publish_active(app: &App) {
    let map: HashMap<String, i64> = app.health.lock().active.iter().map(|(n, x)| (n.to_string(), *x)).collect();
    if app.health.lock().published == map {
        return;
    }
    match app.store.set_setting(ACTIVE_NODES_KEY, &map).await {
        Ok(()) => app.health.lock().published = map,
        Err(e) => tracing::warn!(error = %e, "cannot save the active nodes"),
    }
}

/// A connected driver for a node that does not serve its network now.
async fn standby_driver(app: &App, network: &Network, node: &NodeRow) -> std::result::Result<Arc<dyn PaymentDriver>, Probe> {
    if let Some(d) = app.health.lock().standby.get(&node.node_id) {
        return Ok(d.clone());
    }
    let url = node_url(&app.sealer, node).map_err(|e| Probe::Down(e.to_string()))?;
    match tokio::time::timeout(PROBE_TIMEOUT, connect(network, &url, app.deposit_seed(), &app.store)).await {
        Ok(Ok(d)) => {
            app.health.lock().standby.insert(node.node_id, d.clone());
            Ok(d)
        }
        Ok(Err(e)) if app.deposit_seed().is_none() && e.to_string().contains(NO_SEED) => Err(Probe::Skip),
        Ok(Err(e)) => Err(Probe::Down(redact_error(&e.to_string()))),
        Err(_) => Err(Probe::Down(format!("no answer in {} s", PROBE_TIMEOUT.as_secs()))),
    }
}

async fn probe(driver: &Arc<dyn PaymentDriver>) -> Probe {
    match tokio::time::timeout(PROBE_TIMEOUT, driver.tip_height()).await {
        Ok(Ok(h)) => Probe::Height(h),
        Ok(Err(e)) if e.to_string().contains(NO_TIP) => Probe::Up,
        Ok(Err(e)) => Probe::Down(redact_error(&e.to_string())),
        Err(_) => Probe::Down(format!("no answer in {} s", PROBE_TIMEOUT.as_secs())),
    }
}

async fn check_network(app: &App, network: &Network, nodes: &[NodeRow]) {
    let nid = network.network_id;
    let current_driver = app.drivers().get(&network.chain);
    let active = app.health.active(nid).filter(|_| current_driver.is_some());

    let mut results: Vec<(i64, Probe)> = Vec::with_capacity(nodes.len());
    for node in nodes {
        let result = if Some(node.node_id) == active {
            match &current_driver {
                Some(d) => probe(d).await,
                None => Probe::Skip,
            }
        } else {
            match standby_driver(app, network, node).await {
                Ok(d) => probe(&d).await,
                Err(p) => p,
            }
        };
        if matches!(result, Probe::Down(_)) {
            app.health.lock().standby.remove(&node.node_id);
        }
        results.push((node.node_id, result));
    }

    // A node far behind the best one serves stale data: it counts as failed.
    let best = results.iter().filter_map(|(_, p)| if let Probe::Height(h) = p { Some(*h) } else { None }).max();
    let mut healthy = HashSet::new();
    for (node_id, result) in &results {
        let store = &app.store;
        match result {
            Probe::Height(h) => {
                let _ = store.record_node_ok(*node_id, Some(*h)).await;
                let behind = best.unwrap_or(*h) - h;
                if behind > max_lag(&network.chain) {
                    let _ = store.record_node_error(*node_id, &format!("{behind} blocks behind the best node")).await;
                } else {
                    healthy.insert(*node_id);
                }
            }
            Probe::Up => {
                let _ = store.record_node_ok(*node_id, None).await;
                healthy.insert(*node_id);
            }
            Probe::Down(msg) => {
                let _ = store.record_node_error(*node_id, msg).await;
            }
            Probe::Skip => {}
        }
    }
    if let Some(h) = best {
        let _ = app.store.advance_last_block(nid, h).await;
    }

    let order: Vec<i64> = nodes.iter().map(|n| n.node_id).collect();
    let target = {
        let mut st = app.health.lock();
        for (node_id, result) in &results {
            let s = st.streaks.entry(*node_id).or_default();
            match result {
                Probe::Skip => {}
                _ if healthy.contains(node_id) => *s = Streak { ok: s.ok.saturating_add(1), failed: 0 },
                _ => *s = Streak { ok: 0, failed: s.failed.saturating_add(1) },
            }
        }
        let streak = |id: &i64| st.streaks.get(id).copied().unwrap_or_default();
        pick_node(&order, active, &healthy, streak)
    };
    if let Some(target) = target {
        switch(app, network, nodes, active, target).await;
    }
}

/// The node a network moves to: the first healthy one when the current node failed
/// [`FAILOVER_AFTER`] probes in a row, or a node of higher priority that is back for
/// [`FAILBACK_AFTER`] probes. `order` is the priority order.
fn pick_node(order: &[i64], active: Option<i64>, healthy: &HashSet<i64>, streak: impl Fn(&i64) -> Streak) -> Option<i64> {
    match active {
        None => order.iter().find(|id| healthy.contains(id)).copied(),
        Some(cur) => match order.iter().position(|id| *id == cur) {
            Some(pos) if streak(&cur).failed < FAILOVER_AFTER => {
                order[..pos].iter().find(|id| healthy.contains(id) && streak(id).ok >= FAILBACK_AFTER).copied()
            }
            _ => order.iter().find(|id| **id != cur && healthy.contains(id)).copied(),
        },
    }
}

async fn switch(app: &App, network: &Network, nodes: &[NodeRow], from: Option<i64>, to: i64) {
    let Some(node) = nodes.iter().find(|n| n.node_id == to) else { return };
    let Ok(driver) = standby_driver(app, network, node).await else { return };
    let old = app.drivers().get(&network.chain);
    app.replace_driver(driver);
    {
        let mut st = app.health.lock();
        st.standby.remove(&to);
        st.active.insert(network.network_id, to);
        // A healthy node that hands back to a preferred node stays ready.
        if let (Some(from), Some(old)) = (from, old)
            && st.streaks.get(&from).is_some_and(|s| s.failed == 0)
        {
            st.standby.insert(from, old);
        }
    }
    let label = |id: Option<i64>| id.and_then(|id| nodes.iter().find(|n| n.node_id == id)).map(|n| n.label.clone());
    tracing::warn!(network = %network.code, from = ?label(from), to = %node.label, "network moved to another node");
    if app.role.runs_worker() && from.is_some() {
        let data = serde_json::json!({ "from": label(from), "to": node.label, "to_node": to });
        let _ = app.store.insert_audit(None, None, "system", "node.failover", "network", Some(&network.code), &data, None).await;
    }
}

/// Move `user:pass@` out of a URL.
fn split_credentials(raw: &str) -> (String, Option<String>, Option<String>) {
    match url::Url::parse(raw) {
        Ok(mut u) if !u.username().is_empty() => {
            let user = Some(u.username().to_string());
            let pass = u.password().map(str::to_string);
            let _ = u.set_username("");
            let _ = u.set_password(None);
            (u.to_string().trim_end_matches('/').to_string(), user, pass)
        }
        _ => (raw.to_string(), None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connected_nodes() {
        let h = Health::default();
        h.lock().active.insert(7, 70);
        h.lock().standby.insert(71, Arc::new(crate::chains::driver::MockDriver::new("tron")));
        assert!(h.connected(7, 70));
        assert!(h.connected(7, 71));
        assert!(!h.connected(7, 72), "never connected, or refused at connect");
        assert!(!h.connected(8, 70), "active on another network only");
    }

    #[test]
    fn redacts() {
        assert_eq!(redact_url("https://user:pw@node.example.com:8332/"), "https://node.example.com:8332/");
        assert_eq!(redact_url("https://eth-mainnet.g.alchemy.com/v2/abcdefghijklmnop"), "https://eth-mainnet.g.alchemy.com/redacted");
        assert_eq!(split_credentials("http://u:p@127.0.0.1:18443").1.as_deref(), Some("u"));
        assert_eq!(split_api_key("https://api.trongrid.io/?apikey=k1"), ("https://api.trongrid.io".to_string(), Some("k1".to_string())));
        assert_eq!(split_api_key("https://x.example/api/v2?api_key=k2&a=b").1.as_deref(), Some("k2"));
        assert_eq!(split_api_key("http://127.0.0.1:9090").1, None);
    }

    #[test]
    fn redacts_errors() {
        assert_eq!(
            redact_error("error sending request for url (https://eth-mainnet.g.alchemy.com/v2/abcdefghijklmnop): timed out"),
            "error sending request for url (https://eth-mainnet.g.alchemy.com/redacted): timed out"
        );
        assert_eq!(redact_error("http://u:secret@10.0.0.2:8332/ refused"), "http://10.0.0.2:8332/ refused");
        assert_eq!(redact_error("no url here"), "no url here");
        assert!(!redact_error("a https://x.example/?key=SECRET and wss://y.example/?token=T").contains("SECRET"));
    }

    fn streaks(list: &[(i64, u32, u32)]) -> impl Fn(&i64) -> Streak + '_ {
        move |id| list.iter().find(|(n, ..)| n == id).map(|(_, ok, failed)| Streak { ok: *ok, failed: *failed }).unwrap_or_default()
    }

    #[test]
    fn failover_and_back() {
        let order = [1, 2, 3];
        let all: HashSet<i64> = [1, 2, 3].into();
        // No node yet: the first healthy one.
        assert_eq!(pick_node(&order, None, &[2, 3].into(), streaks(&[])), Some(2));
        // One failure is not enough.
        assert_eq!(pick_node(&order, Some(1), &[2, 3].into(), streaks(&[(1, 0, 1), (2, 1, 0)])), None);
        // Two failures in a row: the next healthy node in priority order.
        assert_eq!(pick_node(&order, Some(1), &[3].into(), streaks(&[(1, 0, 2), (3, 5, 0)])), Some(3));
        // Nothing healthy: stay.
        assert_eq!(pick_node(&order, Some(1), &HashSet::new(), streaks(&[(1, 0, 4)])), None);
        // The preferred node is back once: wait. Twice: move back.
        assert_eq!(pick_node(&order, Some(3), &all, streaks(&[(1, 1, 0), (3, 9, 0)])), None);
        assert_eq!(pick_node(&order, Some(3), &all, streaks(&[(1, 2, 0), (2, 2, 0), (3, 9, 0)])), Some(1));
        // The node in use is the preferred one: stay.
        assert_eq!(pick_node(&order, Some(1), &all, streaks(&[(1, 3, 0), (2, 3, 0)])), None);
        // The node in use was removed: move.
        assert_eq!(pick_node(&order, Some(9), &all, streaks(&[])), Some(1));
    }
}
