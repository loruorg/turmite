use askama::Template;
use axum::{
    Form, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use std::collections::HashMap;

use chrono::Utc;
use serde::Deserialize;

use super::{DashResult, Shell, User, err_redirect, ok_redirect, page};
use crate::{
    app::App,
    auth,
    chains::registry::{self, redact_error, redact_url},
    db::{AuditRow, Page},
    error::Error,
    http::views::{ago, datetime},
    updates::{self, Version},
};

pub fn routes() -> Router<App> {
    Router::new()
        .route("/admin/users", get(users).post(create_user))
        .route("/admin/users/{uid}/role", post(set_role))
        .route("/admin/users/{uid}/disable", post(disable_user))
        .route("/admin/users/{uid}/reset-2fa", post(reset_2fa))
        .route("/admin/networks", get(networks))
        .route("/admin/networks/{nid}", post(save_network))
        .route("/admin/networks/{nid}/nodes", post(add_node))
        .route("/admin/networks/{nid}/tokens", post(save_tokens))
        .route("/admin/networks/{nid}/tokens/add", post(add_token))
        .route("/admin/nodes/{node}/delete", post(delete_node))
        .route("/admin/audit", get(audit))
        .route("/admin/updates", get(updates_page))
        .route("/admin/updates/check", post(check_updates))
        .route("/admin/updates/settings", post(save_update_settings))
        .route("/admin/updates/dismiss", post(dismiss_update))
}

// ---- users ---------------------------------------------------------------------

struct UserLine {
    id: i64,
    email: String,
    role: String,
    totp: bool,
    disabled: bool,
    last_login: String,
    is_root: bool,
}

#[derive(Template)]
#[template(path = "dashboard/users.html")]
struct UsersPage {
    shell: Shell,
    users: Vec<UserLine>,
    is_root: bool,
}

async fn users(State(app): State<App>, user: User) -> DashResult {
    user.require_admin()?;
    let users = app
        .store
        .list_users()
        .await?
        .into_iter()
        .map(|u| UserLine {
            id: u.user_id,
            is_root: u.is_root(),
            email: u.email,
            role: u.role,
            totp: u.totp_enabled,
            disabled: u.disabled_at.is_some(),
            last_login: u.last_login_at.map(ago).unwrap_or_else(|| "Never".into()),
        })
        .collect();
    let is_root = user.user.is_root();
    let shell = Shell::new(&app, &user, None, "users", "Users").await;
    Ok(page(&app, &UsersPage { shell, users, is_root }))
}

#[derive(Deserialize)]
struct NewUserForm {
    email: String,
    password: String,
    role: String,
    #[serde(default)]
    totp: String,
}

async fn create_user(State(app): State<App>, user: User, Form(f): Form<NewUserForm>) -> DashResult {
    user.require_admin()?;
    let back = app.path("/dashboard/admin/users");
    if let Err(e) = user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    if !matches!(f.role.as_str(), "admin" | "member") || (f.role == "admin" && !user.user.is_root()) {
        return Ok(err_redirect(&app, &back, "Only the root account adds admins."));
    }
    if !f.email.contains('@') {
        return Ok(err_redirect(&app, &back, "Enter a valid email address."));
    }
    if let Err(e) = auth::check_password_policy(&f.password) {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    let hash = auth::hash_password(&f.password).await?;
    match app.store.create_user(&f.email, &hash, &f.role).await {
        Ok(id) => {
            user.audit(&app, None, "user.create", "user", Some(&id.to_string()), serde_json::json!({ "email": f.email, "role": f.role }))
                .await;
            Ok(ok_redirect(&app, &back, "User added. Give them the password over a safe channel and add them to projects."))
        }
        Err(e) => Ok(err_redirect(&app, &back, &e.public_message())),
    }
}

#[derive(Deserialize)]
struct RoleForm {
    role: String,
    #[serde(default)]
    totp: String,
}

async fn set_role(State(app): State<App>, user: User, Path(uid): Path<i64>, Form(f): Form<RoleForm>) -> DashResult {
    user.require_root()?;
    if !matches!(f.role.as_str(), "admin" | "member") {
        return Err(Error::BadRequest("Unknown role.".into()).into());
    }
    if f.role == "admin"
        && let Err(e) = user.check_totp(&app, Some(&f.totp)).await
    {
        return Ok(err_redirect(&app, &app.path("/dashboard/admin/users"), &e.public_message()));
    }
    app.store.set_user_role(uid, &f.role).await?;
    user.audit(&app, None, "user.role", "user", Some(&uid.to_string()), serde_json::json!({ "role": f.role })).await;
    Ok(ok_redirect(&app, &app.path("/dashboard/admin/users"), "Role changed."))
}

#[derive(Deserialize)]
struct DisableForm {
    disabled: String,
}

async fn disable_user(State(app): State<App>, user: User, Path(uid): Path<i64>, Form(f): Form<DisableForm>) -> DashResult {
    user.require_admin()?;
    let target = app.store.user(uid).await?;
    if target.is_admin() && !user.user.is_root() {
        return Err(Error::Forbidden("Only the root account can disable admins.".into()).into());
    }
    if target.user_id == user.user.user_id {
        return Err(Error::BadRequest("You cannot disable your own account.".into()).into());
    }
    let disabled = f.disabled == "1";
    app.store.set_user_disabled(uid, disabled).await?;
    user.audit(&app, None, if disabled { "user.disable" } else { "user.enable" }, "user", Some(&uid.to_string()), serde_json::json!({}))
        .await;
    Ok(ok_redirect(&app, &app.path("/dashboard/admin/users"), if disabled { "User disabled." } else { "User enabled." }))
}

#[derive(Deserialize)]
struct TotpForm {
    #[serde(default)]
    totp: String,
}

async fn reset_2fa(State(app): State<App>, user: User, Path(uid): Path<i64>, Form(f): Form<TotpForm>) -> DashResult {
    user.require_root()?;
    let back = app.path("/dashboard/admin/users");
    if let Err(e) = user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    app.store.disable_totp(uid).await?;
    app.store.delete_user_sessions(uid).await?;
    user.audit(&app, None, "user.2fa_reset", "user", Some(&uid.to_string()), serde_json::json!({})).await;
    Ok(ok_redirect(&app, &back, "Two-factor reset. The user signs in with their password and sets it up again."))
}

// ---- networks ------------------------------------------------------------------

struct NodeLine {
    id: i64,
    label: String,
    url: String,
    /// ok, error, unknown or off.
    state: &'static str,
    /// The worker reads the chain through this node.
    in_use: bool,
    checked: String,
    error: String,
    height: String,
    lag: String,
    behind: bool,
}

struct TokenLine {
    id: i64,
    ticker: String,
    symbol: String,
    contract: String,
    decimals: i32,
    enabled: bool,
}

struct NetworkLine {
    id: i64,
    code: String,
    name: String,
    enabled: bool,
    confirmations: i32,
    connected: bool,
    last_block: String,
    last_block_age: String,
    stale: bool,
    tickers: String,
    nodes: Vec<NodeLine>,
    tokens: Vec<TokenLine>,
    /// The family has token contracts (EVM, Tron, Solana).
    has_tokens: bool,
    hint: &'static str,
}

#[derive(Template)]
#[template(path = "dashboard/networks.html")]
struct NetworksPage {
    shell: Shell,
    networks: Vec<NetworkLine>,
    mainnet: bool,
    assets: Vec<(String, String)>,
}

/// Families whose drivers handle token contracts.
fn has_tokens(family: &str) -> bool {
    matches!(family, "evm" | "tron" | "solana")
}

/// Format check of a token contract address for a family.
fn contract_ok(family: &str, contract: &str) -> bool {
    let base58 = |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric() && !matches!(c, '0' | 'O' | 'I' | 'l'));
    match family {
        "evm" => contract.len() == 42 && contract.starts_with("0x") && contract[2..].chars().all(|c| c.is_ascii_hexdigit()),
        "tron" => contract.len() == 34 && contract.starts_with('T') && base58(contract),
        "solana" => (32..=44).contains(&contract.len()) && base58(contract),
        _ => false,
    }
}

fn node_hint(family: &str) -> &'static str {
    match family {
        "bitcoin" => "bitcoind JSON-RPC, e.g. http://user:password@bitcoind:8332",
        "monero" => "monero-wallet-rpc, e.g. http://monero-wallet-rpc:18083",
        "evm" => "JSON-RPC endpoint, e.g. https://eth-mainnet.g.alchemy.com/v2/KEY",
        "solana" => "JSON-RPC endpoint, e.g. https://api.mainnet-beta.solana.com",
        "tron" => "Full node HTTP API, e.g. https://api.trongrid.io",
        "ton" => "toncenter HTTP API v2, e.g. https://toncenter.com/api/v2",
        _ => "Node URL",
    }
}

async fn networks(State(app): State<App>, user: User) -> DashResult {
    user.require_admin()?;
    let nodes = app.store.list_nodes(None).await?;
    let ans = app.store.list_asset_networks().await?;
    let drivers = app.drivers();
    // The worker's nodes; this process's own when no worker wrote them yet.
    let in_use: HashMap<String, i64> = app.store.setting(registry::ACTIVE_NODES_KEY).await?.unwrap_or_default();
    let now = Utc::now();
    let recent = chrono::Duration::minutes(10);
    let networks = app
        .store
        .list_networks()
        .await?
        .into_iter()
        .map(|n| {
            let mine: Vec<_> = nodes.iter().filter(|x| x.network_id == n.network_id).collect();
            let active = in_use.get(&n.network_id.to_string()).copied().or_else(|| app.health.active(n.network_id));
            let fresh = |x: &crate::db::NodeRow| x.is_enabled && x.last_ok_at.is_some_and(|t| now - t < recent);
            let best = mine.iter().filter(|x| fresh(x)).filter_map(|x| x.last_height).max();
            NetworkLine {
                id: n.network_id,
                connected: drivers.get(&n.chain).is_some(),
                tickers: ans
                    .iter()
                    .filter(|a| a.network.network_id == n.network_id && a.is_enabled)
                    .map(|a| a.ticker.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
                last_block: n.last_block.map(|b| b.to_string()).unwrap_or_default(),
                last_block_age: n.last_block_at.map(ago).unwrap_or_default(),
                stale: n.is_enabled && !mine.is_empty() && n.last_block_at.is_some_and(|t| now - t > registry::stale_after(&n.chain)),
                nodes: mine
                    .iter()
                    .map(|x| {
                        let error = match (x.last_error_at, x.last_ok_at) {
                            (Some(e), Some(o)) if e > o => x.last_error.as_deref().map(redact_error).unwrap_or_default(),
                            (Some(_), None) => x.last_error.as_deref().map(redact_error).unwrap_or_default(),
                            _ => String::new(),
                        };
                        let lag = match (best, x.last_height) {
                            (Some(b), Some(h)) if fresh(x) => b - h,
                            _ => 0,
                        };
                        NodeLine {
                            id: x.node_id,
                            label: x.label.clone(),
                            url: x.url_display.clone(),
                            state: if !x.is_enabled {
                                "off"
                            } else if !error.is_empty() {
                                "error"
                            } else if x.last_ok_at.is_some() {
                                "ok"
                            } else {
                                "unknown"
                            },
                            in_use: active == Some(x.node_id),
                            checked: x.last_ok_at.max(x.last_error_at).map(ago).unwrap_or_default(),
                            error,
                            height: x.last_height.map(|h| h.to_string()).unwrap_or_default(),
                            lag: if lag > 0 { format!("{lag} behind") } else { String::new() },
                            behind: lag > registry::max_lag(&n.chain),
                        }
                    })
                    .collect(),
                tokens: ans
                    .iter()
                    .filter(|a| a.network.network_id == n.network_id)
                    .map(|a| TokenLine {
                        id: a.asset_network_id,
                        ticker: a.ticker.clone(),
                        symbol: a.asset.symbol.clone(),
                        contract: a.contract_address.clone().unwrap_or_default(),
                        decimals: a.decimals,
                        enabled: a.is_enabled,
                    })
                    .collect(),
                has_tokens: has_tokens(&n.family),
                hint: node_hint(&n.family),
                code: n.code,
                name: n.name,
                enabled: n.is_enabled,
                confirmations: n.confirmations_required,
            }
        })
        .collect();
    let assets = app.store.list_asset_symbols().await?;
    let shell = Shell::new(&app, &user, None, "networks", "Networks").await;
    Ok(page(&app, &NetworksPage { shell, networks, mainnet: app.config.mode.is_mainnet(), assets }))
}

#[derive(Deserialize)]
struct NetworkForm {
    confirmations: i32,
    #[serde(default)]
    enabled: Option<String>,
    #[serde(default)]
    totp: String,
}

async fn save_network(State(app): State<App>, user: User, Path(nid): Path<i64>, Form(f): Form<NetworkForm>) -> DashResult {
    user.require_admin()?;
    let back = app.path("/dashboard/admin/networks");
    let network = app.store.network(nid).await?;
    if f.confirmations < 0 || f.confirmations > 10_000 {
        return Ok(err_redirect(&app, &back, "Confirmations must be 0 to 10000."));
    }
    // Fewer confirmations raise the risk of crediting a reorged deposit.
    if f.confirmations < network.confirmations_required
        && let Err(e) = user.check_totp(&app, Some(&f.totp)).await
    {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    app.store.set_network_confirmations(nid, f.confirmations).await?;
    app.store.set_network_enabled(nid, f.enabled.is_some()).await?;
    app.store.bump_nodes_version().await?;
    user.audit(
        &app,
        None,
        "network.save",
        "network",
        Some(&network.code),
        serde_json::json!({ "confirmations": f.confirmations, "enabled": f.enabled.is_some() }),
    )
    .await;
    Ok(ok_redirect(&app, &back, &format!("{} saved.", network.name)))
}

#[derive(Deserialize)]
struct NodeForm {
    url: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    totp: String,
}

async fn add_node(State(app): State<App>, user: User, Path(nid): Path<i64>, Form(f): Form<NodeForm>) -> DashResult {
    user.require_admin()?;
    let back = app.path("/dashboard/admin/networks");
    if let Err(e) = user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    let network = app.store.network(nid).await?;
    let url = f.url.trim();
    let parsed = url::Url::parse(url).map_err(|_| Error::BadRequest("Enter a full URL with http:// or https://.".into()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Ok(err_redirect(&app, &back, "Enter a full URL with http:// or https://."));
    }
    let label = if f.label.trim().is_empty() { parsed.host_str().unwrap_or("node").to_string() } else { f.label.trim().to_string() };
    let existing = app.store.list_nodes(Some(nid)).await?.len() as i32;
    let id = app.store.add_node(nid, &label, &app.sealer.seal_str("node_url", url), &redact_url(url), existing).await?;
    user.audit(
        &app,
        None,
        "node.add",
        "node",
        Some(&id.to_string()),
        serde_json::json!({ "network": network.code, "url": redact_url(url) }),
    )
    .await;
    let _ = app.refresh_drivers().await;
    let connected = app.drivers().get(&network.chain).is_some();
    if connected {
        Ok(ok_redirect(&app, &back, &format!("Node added. {} is connected.", network.name)))
    } else {
        Ok(err_redirect(&app, &back, &format!("Node added, but {} did not connect. See the node error below.", network.name)))
    }
}

async fn delete_node(State(app): State<App>, user: User, Path(node): Path<i64>, Form(f): Form<TotpForm>) -> DashResult {
    user.require_admin()?;
    let back = app.path("/dashboard/admin/networks");
    if let Err(e) = user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    app.store.delete_node(node).await?;
    user.audit(&app, None, "node.delete", "node", Some(&node.to_string()), serde_json::json!({})).await;
    let _ = app.refresh_drivers().await;
    Ok(ok_redirect(&app, &back, "Node removed."))
}

/// Turn the assets of a network on or off. The form has a `t<id>` field per checked asset.
async fn save_tokens(State(app): State<App>, user: User, Path(nid): Path<i64>, Form(f): Form<HashMap<String, String>>) -> DashResult {
    user.require_admin()?;
    let back = app.path("/dashboard/admin/networks");
    let network = app.store.network(nid).await?;
    let changes: Vec<(i64, String, bool)> = app
        .store
        .list_asset_networks()
        .await?
        .into_iter()
        .filter(|a| a.network.network_id == nid)
        .filter_map(|a| {
            let on = f.contains_key(&format!("t{}", a.asset_network_id));
            (on != a.is_enabled).then_some((a.asset_network_id, a.ticker, on))
        })
        .collect();
    if changes.is_empty() {
        return Ok(ok_redirect(&app, &back, "Nothing changed."));
    }
    if let Err(e) = user.check_totp(&app, f.get("totp").map(String::as_str)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    for (id, _, on) in &changes {
        app.store.set_asset_network_enabled(*id, *on).await?;
    }
    let data: Vec<_> = changes.iter().map(|(_, t, on)| serde_json::json!({ "ticker": t, "enabled": on })).collect();
    user.audit(&app, None, "token.toggle", "network", Some(&network.code), serde_json::json!({ "changes": data })).await;
    Ok(ok_redirect(&app, &back, &format!("{} tokens saved.", network.name)))
}

#[derive(Deserialize)]
struct TokenForm {
    symbol: String,
    ticker: String,
    decimals: String,
    contract: String,
    #[serde(default)]
    totp: String,
}

/// Add a token contract for a known asset (for example USDC on a network without it).
async fn add_token(State(app): State<App>, user: User, Path(nid): Path<i64>, Form(f): Form<TokenForm>) -> DashResult {
    user.require_admin()?;
    let back = app.path("/dashboard/admin/networks");
    let network = app.store.network(nid).await?;
    if !has_tokens(&network.family) {
        return Ok(err_redirect(&app, &back, &format!("{} has no token contracts.", network.name)));
    }
    let ticker = f.ticker.trim().to_ascii_lowercase();
    let contract = f.contract.trim();
    let Ok(decimals) = f.decimals.trim().parse::<i32>() else {
        return Ok(err_redirect(&app, &back, "Decimals must be a number from 0 to 30."));
    };
    if !(0..=30).contains(&decimals) {
        return Ok(err_redirect(&app, &back, "Decimals must be a number from 0 to 30."));
    }
    if !(2..=20).contains(&ticker.len()) || !ticker.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
        return Ok(err_redirect(&app, &back, "The ticker has 2 to 20 letters and digits, for example usdcbase."));
    }
    if !contract_ok(&network.family, contract) {
        return Ok(err_redirect(&app, &back, &format!("This is not a {} contract address.", network.name)));
    }
    if let Some(driver) = app.drivers().get(&network.chain)
        && !driver.validate_address(contract).await.unwrap_or(false)
    {
        return Ok(err_redirect(&app, &back, &format!("{} does not accept this contract address.", network.name)));
    }
    let existing = app.store.list_asset_networks().await?;
    if existing.iter().any(|a| a.ticker == ticker) {
        return Ok(err_redirect(&app, &back, &format!("The ticker {ticker} is in use.")));
    }
    if existing.iter().any(|a| a.network.network_id == nid && a.asset.symbol.eq_ignore_ascii_case(&f.symbol)) {
        return Ok(err_redirect(&app, &back, &format!("{} is already on {}.", f.symbol, network.name)));
    }
    if let Err(e) = user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    match app.store.upsert_token(nid, &f.symbol, &ticker, decimals, contract, None).await {
        Ok(id) => {
            let data = serde_json::json!({ "network": network.code, "symbol": f.symbol, "ticker": ticker, "decimals": decimals, "contract": contract });
            user.audit(&app, None, "token.add", "asset_network", Some(&id.to_string()), data).await;
            Ok(ok_redirect(&app, &back, &format!("{ticker} added to {}.", network.name)))
        }
        Err(e) => Ok(err_redirect(&app, &back, &e.public_message())),
    }
}

// ---- audit ---------------------------------------------------------------------

#[derive(Deserialize)]
struct AuditQuery {
    page: Option<i64>,
}

#[derive(Template)]
#[template(path = "dashboard/audit.html")]
struct AuditPage {
    shell: Shell,
    rows: Vec<AuditRow>,
    page: i64,
}

async fn audit(State(app): State<App>, user: User, Query(q): Query<AuditQuery>) -> DashResult {
    user.require_admin()?;
    let page_no = q.page.unwrap_or(0).max(0);
    let rows = app.store.list_audit(None, Page::new(100, page_no)).await?;
    let shell = Shell::new(&app, &user, None, "audit", "Audit log").await;
    Ok(page(&app, &AuditPage { shell, rows, page: page_no }))
}

// ---- updates -------------------------------------------------------------------

struct AdvisoryLine {
    id: String,
    url: String,
    severity: String,
    summary: String,
    ranges: String,
    patched: String,
    published: String,
    affects: bool,
}

#[derive(Template)]
#[template(path = "dashboard/updates.html")]
struct UpdatesPage {
    shell: Shell,
    version: &'static str,
    dev: bool,
    enabled: bool,
    checked: String,
    error: String,
    latest: String,
    latest_url: String,
    latest_date: String,
    newer: bool,
    advisories: Vec<AdvisoryLine>,
    affected: bool,
    command: &'static str,
    releases: &'static str,
}

async fn updates_page(State(app): State<App>, user: User) -> DashResult {
    user.require_admin()?;
    let cur = Version::parse(updates::current());
    let state = updates::state(&app).await;
    let latest = state.latest.clone();
    let advisories: Vec<AdvisoryLine> = state
        .advisories
        .iter()
        .map(|a| AdvisoryLine {
            affects: cur.as_ref().is_some_and(|v| a.affects(v)),
            id: a.id.clone(),
            url: a.url.clone(),
            severity: a.severity.clone(),
            summary: a.summary.clone(),
            ranges: if a.ranges.is_empty() { "All".into() } else { a.ranges.join("; ") },
            patched: a.patched.join("; "),
            published: a.published_at.map(|t| t.format("%Y-%m-%d").to_string()).unwrap_or_default(),
        })
        .collect();
    let shell = Shell::new(&app, &user, None, "updates", "Updates").await;
    let tpl = UpdatesPage {
        shell,
        version: updates::current(),
        dev: cur.as_ref().is_some_and(Version::is_dev),
        enabled: updates::enabled(&app).await,
        checked: state.checked_at.map(|t| format!("{} ({})", ago(t), datetime(t))).unwrap_or_default(),
        error: state.error.unwrap_or_default(),
        newer: latest.as_ref().and_then(|l| Version::parse(&l.version)).zip(cur.clone()).is_some_and(|(l, c)| l > c),
        latest: latest.as_ref().map(|l| l.version.clone()).unwrap_or_default(),
        latest_url: latest.as_ref().map(|l| l.url.clone()).unwrap_or_default(),
        latest_date: latest.as_ref().and_then(|l| l.published_at).map(|t| t.format("%Y-%m-%d").to_string()).unwrap_or_default(),
        affected: advisories.iter().any(|a| a.affects),
        advisories,
        command: updates::UPGRADE_COMMAND,
        releases: updates::RELEASES_PAGE,
    };
    Ok(page(&app, &tpl))
}

async fn check_updates(State(app): State<App>, user: User) -> DashResult {
    user.require_admin()?;
    let back = app.path("/dashboard/admin/updates");
    let state = updates::check(&app).await;
    match (state.error, state.latest) {
        (Some(e), _) => Ok(err_redirect(&app, &back, &format!("The check failed: {e}"))),
        (None, Some(l)) => Ok(ok_redirect(&app, &back, &format!("Checked. The latest release is {}.", l.version))),
        (None, None) => Ok(ok_redirect(&app, &back, "Checked. There is no release yet.")),
    }
}

#[derive(Deserialize)]
struct UpdateSettingsForm {
    #[serde(default)]
    enabled: Option<String>,
    #[serde(default)]
    totp: String,
}

async fn save_update_settings(State(app): State<App>, user: User, Form(f): Form<UpdateSettingsForm>) -> DashResult {
    user.require_admin()?;
    let back = app.path("/dashboard/admin/updates");
    let on = f.enabled.is_some();
    if on == updates::enabled(&app).await {
        return Ok(ok_redirect(&app, &back, "Nothing changed."));
    }
    // Without the check, admins do not see security advisories.
    if !on && let Err(e) = user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    updates::set_enabled(&app, on).await?;
    user.audit(&app, None, "updates.settings", "settings", Some("update_check"), serde_json::json!({ "enabled": on })).await;
    Ok(ok_redirect(&app, &back, if on { "Update checks are on." } else { "Update checks are off." }))
}

async fn dismiss_update(State(app): State<App>, user: User) -> DashResult {
    user.require_admin()?;
    if let Some(latest) = updates::state(&app).await.latest {
        updates::dismiss(&app, &latest.version).await?;
    }
    Ok(ok_redirect(&app, &app.path("/dashboard/admin/updates"), "Hidden until the next release."))
}
