use askama::Template;
use axum::{
    Form, Router,
    extract::{Path, State},
    routing::{get, post},
};
use serde::Deserialize;

use super::{DashResult, Proj, ProjectRole, Shell, err_redirect, ok_redirect, page};
use crate::{
    app::App,
    chains::{btc, driver::WalletContext, xmr},
    error::Error,
};

pub fn routes() -> Router<App> {
    let r = Router::new()
        .route("/p/{pid}/wallets", get(list))
        .route("/p/{pid}/wallets/{nid}", post(save))
        .route("/p/{pid}/wallets/{nid}/bitcoin", post(save_bitcoin))
        .route("/p/{pid}/wallets/{nid}/monero", post(save_monero))
        .route("/p/{pid}/wallets/{nid}/toggle", post(toggle));
    #[cfg(feature = "evm")]
    let r = r.route("/p/{pid}/wallets/evm", post(evm::save)).route("/p/{pid}/wallets/evm/{nid}/deploy", post(evm::deploy));
    r
}

struct WalletLine {
    network_id: i64,
    name: String,
    family: String,
    assets: String,
    address: String,
    extra: String,
    enabled: bool,
    configured: bool,
    connected: bool,
    custody: &'static str,
    /// Bitcoin: account key, and its first receive addresses.
    xpub: String,
    xpub_short: String,
    check_addresses: Vec<String>,
    /// Monero: restore height, and whether a view key is saved.
    restore_height: String,
    has_view_key: bool,
}

/// The EVM networks of a project: one cold wallet and one factory for all of them.
#[derive(Default)]
struct EvmBlock {
    cold: String,
    factory: String,
    deploy_to: String,
    deploy_data: String,
    has_hot: bool,
    networks: Vec<EvmNet>,
    recent: Vec<EvmTxLine>,
}

struct EvmNet {
    network_id: i64,
    name: String,
    assets: String,
    enabled: bool,
    configured: bool,
    connected: bool,
    /// "yes", "no" or "" (unknown).
    deployed: &'static str,
    deploying: bool,
}

struct EvmTxLine {
    network: String,
    kind: &'static str,
    status: String,
    txid: String,
    href: String,
    fee: String,
    when: String,
}

#[derive(Template)]
#[template(path = "dashboard/wallets.html")]
struct WalletsPage {
    shell: Shell,
    pid: i64,
    lines: Vec<WalletLine>,
    evm: Option<EvmBlock>,
    can_edit: bool,
}

/// One sentence on who holds the funds on this family.
pub fn custody(family: &str) -> &'static str {
    match family {
        "bitcoin" => "Deposits go to addresses of your wallet. The server holds no Bitcoin key.",
        "monero" => "Deposits go to subaddresses of your wallet. The server holds only the view key.",
        "evm" => "Each payment gets a deposit contract that can only send to this cold wallet.",
        _ => "Each payment gets its own address. Confirmed funds are swept to this address.",
    }
}

/// Start and end of a long key, for display.
fn shorten(s: &str) -> String {
    if s.len() <= 24 { s.to_string() } else { format!("{}…{}", &s[..12], &s[s.len() - 8..]) }
}

async fn list(State(app): State<App>, p: Proj) -> DashResult {
    let pid = p.project.project_id;
    let wallets = app.store.project_wallets(pid).await?;
    let check = app.store.wallet_check_addresses(pid).await?;
    let view_keys = app.store.wallets_with_view_key(pid).await?;
    let ans = app.store.list_asset_networks().await?;
    let drivers = app.drivers();
    let networks: Vec<_> = app.store.list_networks().await?.into_iter().filter(|n| n.is_enabled).collect();
    let assets_of = |network_id: i64| {
        ans.iter()
            .filter(|a| a.network.network_id == network_id && a.is_enabled)
            .map(|a| a.asset.symbol.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let evm_block = cfg!(feature = "evm");
    let lines = networks
        .iter()
        .filter(|n| !(evm_block && n.family == "evm"))
        .map(|n| {
            let w = wallets.iter().find(|w| w.network_id == n.network_id);
            let xpub = w.and_then(|w| w.xpub.clone()).unwrap_or_default();
            let has_view_key = view_keys.contains(&n.network_id);
            let configured = match n.family.as_str() {
                "bitcoin" => !xpub.is_empty(),
                "monero" => has_view_key,
                _ => w.is_some(),
            };
            WalletLine {
                network_id: n.network_id,
                name: n.name.clone(),
                family: n.family.clone(),
                assets: assets_of(n.network_id),
                address: w.and_then(|w| w.address.clone()).unwrap_or_default(),
                extra: w.and_then(|w| w.address_extra_id.clone()).unwrap_or_default(),
                enabled: w.is_some_and(|w| w.is_enabled),
                configured,
                connected: drivers.get(&n.chain).is_some(),
                custody: custody(&n.family),
                xpub_short: shorten(&xpub),
                xpub,
                check_addresses: check.iter().find(|(id, _)| *id == n.network_id).map(|(_, a)| a.clone()).unwrap_or_default(),
                restore_height: w.and_then(|w| w.restore_height).map(|h| h.to_string()).unwrap_or_default(),
                has_view_key,
            }
        })
        .collect();
    #[cfg(feature = "evm")]
    let evm = evm::block(&app, pid, &networks, &wallets, &assets_of).await?;
    #[cfg(not(feature = "evm"))]
    let evm: Option<EvmBlock> = None;
    let shell = Shell::new(&app, &p.user, Some(&p.project), "wallets", "Wallets").await;
    Ok(page(&app, &WalletsPage { shell, pid, lines, evm, can_edit: p.role >= ProjectRole::Admin }))
}

#[derive(Deserialize)]
struct WalletForm {
    address: String,
    #[serde(default)]
    extra_id: String,
    #[serde(default)]
    totp: String,
}

async fn save(State(app): State<App>, p: Proj, Path((_pid, nid)): Path<(i64, i64)>, Form(f): Form<WalletForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/wallets", p.base(&app));
    if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    let network = app.store.network(nid).await.map_err(|_| Error::NotFound)?;
    if matches!(network.family.as_str(), "bitcoin" | "monero") {
        return Ok(err_redirect(&app, &back, &format!("{} needs a watch-only wallet. Use its own form.", network.name)));
    }
    let address = f.address.trim();
    match crate::addresses::validate(&app, &network, address).await {
        Ok(true) => {}
        Ok(false) => return Ok(err_redirect(&app, &back, &format!("This is not a valid {} address.", network.name))),
        Err(e) => {
            tracing::warn!(network = %network.code, error = %crate::chains::registry::redact_error(&e.to_string()), "address check failed");
            return Ok(err_redirect(
                &app,
                &back,
                &format!("The {} node is not available, so Turmite cannot check the address. Try again later.", network.name),
            ));
        }
    }
    let extra = Some(f.extra_id.trim()).filter(|s| !s.is_empty());
    app.store.upsert_project_wallet_address(p.project.project_id, nid, address, extra).await?;
    p.audit(&app, "wallet.set", "network", Some(&network.code), serde_json::json!({ "address": address, "extra_id": extra })).await;
    let mut message = format!("{} wallet saved.", network.name);
    if let Some(driver) = app.drivers().get(&network.chain) {
        let w = crate::addresses::wallet_context(&app, p.project.project_id, nid).await?;
        match driver.prepare_wallet(&w).await {
            Ok(notes) => notes.iter().for_each(|n| message.push_str(&format!(" {n}"))),
            Err(e) => return Ok(err_redirect(&app, &back, &format!("Saved, but the wallet check failed: {}", e.public_message()))),
        }
    }
    Ok(ok_redirect(&app, &back, &message))
}

/// Message for a wallet check that failed: the reason when the operator can fix it.
fn check_failed(e: &Error) -> String {
    match e {
        Error::BadRequest(m) => m.clone(),
        _ => {
            e.log();
            "The node did not accept the wallet. Check the node under Networks and try again.".into()
        }
    }
}

#[derive(Deserialize)]
struct BitcoinForm {
    xpub: String,
    #[serde(default)]
    totp: String,
}

/// Save a Bitcoin account key. With a node connected, the watch-only wallet is made first and
/// the key is saved only when the node accepts it.
async fn save_bitcoin(State(app): State<App>, p: Proj, Path((_pid, nid)): Path<(i64, i64)>, Form(f): Form<BitcoinForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/wallets", p.base(&app));
    if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    let network = app.store.network(nid).await.map_err(|_| Error::NotFound)?;
    if network.family != "bitcoin" {
        return Err(Error::NotFound.into());
    }
    let xpub = match btc::parse_xpub(&f.xpub, app.config.mode.is_mainnet()) {
        Ok(x) => x,
        Err(m) => return Ok(err_redirect(&app, &back, &m)),
    };
    let pid = p.project.project_id;
    let mut addresses = Vec::new();
    if let Some(driver) = app.drivers().get(&network.chain) {
        let w = WalletContext { project_id: pid, xpub: Some(xpub.clone()), ..Default::default() };
        match driver.prepare_wallet(&w).await {
            Ok(a) => addresses = a,
            Err(e) => return Ok(err_redirect(&app, &back, &check_failed(&e))),
        }
    }
    app.store.set_project_wallet_keys(pid, nid, Some(&xpub), None, None, None).await?;
    app.store.set_wallet_check_addresses(pid, nid, &addresses).await?;
    app.store.set_project_wallet_enabled(pid, nid, true).await?;
    p.audit(&app, "wallet.set", "network", Some(&network.code), serde_json::json!({ "xpub": shorten(&xpub) })).await;
    let message = if addresses.is_empty() {
        format!("{} account key saved. Connect a node under Networks, then save again to see the first addresses.", network.name)
    } else {
        format!("{} account key saved. Compare the first addresses with your wallet.", network.name)
    };
    Ok(ok_redirect(&app, &back, &message))
}

#[derive(Deserialize)]
struct MoneroForm {
    address: String,
    view_key: String,
    #[serde(default)]
    restore_height: String,
    #[serde(default)]
    totp: String,
}

/// Save a Monero view-only wallet. The view key is sealed and never shown again.
async fn save_monero(State(app): State<App>, p: Proj, Path((_pid, nid)): Path<(i64, i64)>, Form(f): Form<MoneroForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/wallets", p.base(&app));
    if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    let network = app.store.network(nid).await.map_err(|_| Error::NotFound)?;
    if network.family != "monero" {
        return Err(Error::NotFound.into());
    }
    let address = f.address.trim();
    if let Err(m) = xmr::check_primary_address(address, app.config.mode.is_mainnet()) {
        return Ok(err_redirect(&app, &back, &m));
    }
    let view_key = zeroize::Zeroizing::new(f.view_key.trim().to_ascii_lowercase());
    if let Err(m) = xmr::check_view_key(address, &view_key) {
        return Ok(err_redirect(&app, &back, &m));
    }
    let driver = app.drivers().get(&network.chain);
    let pid = p.project.project_id;
    // Same address again: keep the wallet file and its height.
    let saved_height =
        app.store.project_wallet(pid, nid).await?.filter(|w| w.address.as_deref() == Some(address)).and_then(|w| w.restore_height);
    let restore_height = match f.restore_height.trim().replace([',', '_', ' '], "") {
        h if h.is_empty() && saved_height.is_some() => saved_height.unwrap_or_default(),
        h if h.is_empty() => match &driver {
            Some(d) => match d.tip_height().await {
                Ok(tip) => tip.max(0),
                Err(_) => {
                    return Ok(err_redirect(
                        &app,
                        &back,
                        "Enter the restore height. The wallet RPC cannot tell the current block height now.",
                    ));
                }
            },
            None => return Ok(err_redirect(&app, &back, "Enter the restore height.")),
        },
        h => match h.parse::<i64>() {
            Ok(h) if h >= 0 => h,
            _ => return Ok(err_redirect(&app, &back, "The restore height is a block number.")),
        },
    };
    if let Some(driver) = &driver {
        let w = WalletContext {
            project_id: pid,
            address: Some(address.to_string()),
            view_key: Some(view_key.clone()),
            restore_height: Some(restore_height),
            ..Default::default()
        };
        if let Err(e) = driver.prepare_wallet(&w).await {
            return Ok(err_redirect(&app, &back, &check_failed(&e)));
        }
    }
    app.store.upsert_project_wallet_address(pid, nid, address, None).await?;
    let sealed = app.sealer.seal_str("view_key", &view_key);
    app.store.set_project_wallet_keys(pid, nid, None, Some(&sealed), Some(restore_height), None).await?;
    p.audit(
        &app,
        "wallet.set",
        "network",
        Some(&network.code),
        serde_json::json!({ "address": address, "restore_height": restore_height }),
    )
    .await;
    let message = match driver {
        Some(_) => format!("{} view-only wallet saved. It scans from block {restore_height}.", network.name),
        None => format!("{} view-only wallet saved. Connect a wallet RPC under Networks to start.", network.name),
    };
    Ok(ok_redirect(&app, &back, &message))
}

#[derive(Deserialize)]
struct ToggleForm {
    enabled: String,
}

async fn toggle(State(app): State<App>, p: Proj, Path((_pid, nid)): Path<(i64, i64)>, Form(f): Form<ToggleForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let on = f.enabled == "1";
    app.store.set_project_wallet_enabled(p.project.project_id, nid, on).await?;
    p.audit(&app, if on { "wallet.enable" } else { "wallet.disable" }, "network", Some(&nid.to_string()), serde_json::json!({})).await;
    Ok(ok_redirect(&app, &format!("{}/wallets", p.base(&app)), if on { "Network turned on." } else { "Network turned off." }))
}

#[cfg(feature = "evm")]
mod evm {
    use std::time::Duration;

    use axum::{
        Form,
        extract::{Path, State},
    };
    use serde::Deserialize;

    use super::{EvmBlock, EvmNet, EvmTxLine};
    use crate::{
        app::App,
        chains::{
            evm::{EvmDriver, key, parse_address},
            evm_factory,
        },
        domain::{Network, ProjectWallet},
        error::Error,
        http::{
            dashboard::{DashResult, Proj, ProjectRole, err_redirect, ok_redirect},
            views::ago,
        },
    };

    const CODE_CHECK_TIMEOUT: Duration = Duration::from_secs(4);

    pub(super) async fn block(
        app: &App,
        pid: i64,
        networks: &[Network],
        wallets: &[ProjectWallet],
        assets_of: &(dyn Fn(i64) -> String + Sync),
    ) -> crate::error::Result<Option<EvmBlock>> {
        let evm_nets: Vec<&Network> = networks.iter().filter(|n| n.family == "evm").collect();
        if evm_nets.is_empty() {
            return Ok(None);
        }
        let cold = evm_nets
            .iter()
            .find_map(|n| wallets.iter().find(|w| w.network_id == n.network_id).and_then(|w| w.address.clone()))
            .unwrap_or_default();
        let cold_addr = parse_address(&cold);
        let factory = cold_addr.map(evm_factory::factory_address);
        let drivers = app.drivers();
        // Ask each connected node for code at the factory, in parallel and with a time limit.
        let mut checks = tokio::task::JoinSet::new();
        if let Some(factory) = factory {
            for n in &evm_nets {
                if let Some(d) = drivers.get(&n.chain) {
                    let id = n.network_id;
                    checks.spawn(async move {
                        let drv = d.as_any().and_then(|a| a.downcast_ref::<EvmDriver>())?;
                        let code = tokio::time::timeout(CODE_CHECK_TIMEOUT, drv.has_code(factory)).await.ok()?.ok()?;
                        Some((id, code))
                    });
                }
            }
        }
        let mut deployed = std::collections::HashMap::new();
        while let Some(r) = checks.join_next().await {
            if let Ok(Some((id, code))) = r {
                deployed.insert(id, code);
            }
        }
        let recent = match factory {
            Some(f) => app.store.recent_evm_txs(&key(&f), 10).await?,
            None => vec![],
        };
        let block = EvmBlock {
            cold: cold_addr.map(|a| a.to_checksum(None)).unwrap_or(cold),
            factory: factory.map(|f| f.to_checksum(None)).unwrap_or_default(),
            deploy_to: evm_factory::CREATE2_DEPLOYER.to_checksum(None),
            deploy_data: cold_addr
                .map(|c| {
                    let data = evm_factory::deploy_factory_tx(c).input.input().cloned().unwrap_or_default();
                    format!("0x{}", alloy::primitives::hex::encode(data))
                })
                .unwrap_or_default(),
            has_hot: crate::hot::address_for_project(app, pid, "evm").await?.is_some(),
            networks: evm_nets
                .iter()
                .map(|n| {
                    let w = wallets.iter().find(|w| w.network_id == n.network_id);
                    EvmNet {
                        network_id: n.network_id,
                        name: n.name.clone(),
                        assets: assets_of(n.network_id),
                        enabled: w.is_some_and(|w| w.is_enabled),
                        configured: w.is_some(),
                        connected: drivers.get(&n.chain).is_some(),
                        deployed: match deployed.get(&n.network_id) {
                            Some(true) => "yes",
                            Some(false) => "no",
                            None => "",
                        },
                        deploying: recent.iter().any(|t| {
                            t.network_id == n.network_id && t.kind == "deploy" && (t.status == "queued" || t.status == "broadcast")
                        }),
                    }
                })
                .collect(),
            recent: recent
                .iter()
                .map(|t| {
                    let net = networks.iter().find(|n| n.network_id == t.network_id);
                    let txid = t.txid.clone().unwrap_or_default();
                    EvmTxLine {
                        network: net.map(|n| n.code.clone()).unwrap_or_default(),
                        kind: if t.kind == "deploy" { "Factory deployment" } else { "Sweep" },
                        status: t.status.clone(),
                        href: net.and_then(|n| n.tx_url(&txid)).filter(|_| !txid.is_empty()).unwrap_or_default(),
                        txid,
                        fee: t
                            .fee_wei
                            .as_deref()
                            .and_then(|w| w.parse().ok())
                            .map(|w| crate::chains::evm::to_decimal(w, 18).round_dp(8).normalize().to_string())
                            .unwrap_or_default(),
                        when: ago(t.created_at),
                    }
                })
                .collect(),
        };
        Ok(Some(block))
    }

    #[derive(Deserialize)]
    pub(super) struct ColdForm {
        address: String,
        #[serde(default)]
        totp: String,
    }

    /// Set the cold wallet of every EVM network of the project.
    pub(super) async fn save(State(app): State<App>, p: Proj, Form(f): Form<ColdForm>) -> DashResult {
        p.require(ProjectRole::Admin)?;
        let back = format!("{}/wallets", p.base(&app));
        if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
            return Ok(err_redirect(&app, &back, &e.public_message()));
        }
        let Some(cold) = parse_address(f.address.trim()) else {
            return Ok(err_redirect(&app, &back, "This is not a valid EVM address. Check the upper and lower case letters."));
        };
        let factory = evm_factory::factory_address(cold);
        let (cold_s, factory_s) = (cold.to_checksum(None), factory.to_checksum(None));
        app.store.set_evm_cold_wallet(p.project.project_id, &cold_s, &factory_s).await?;
        p.audit(&app, "wallet.set", "network", Some("evm"), serde_json::json!({ "address": cold_s, "factory": factory_s })).await;
        Ok(ok_redirect(
            &app,
            &back,
            &format!(
                "Cold wallet saved. Its factory is {factory_s} on every EVM network. Earlier payments still go to the cold wallet they were made for."
            ),
        ))
    }

    #[derive(Deserialize)]
    pub(super) struct DeployForm {
        #[serde(default)]
        totp: String,
    }

    /// Queue a factory deployment. The worker sends it with the project's EVM hot wallet.
    pub(super) async fn deploy(
        State(app): State<App>,
        p: Proj,
        Path((_pid, nid)): Path<(i64, i64)>,
        Form(f): Form<DeployForm>,
    ) -> DashResult {
        p.require(ProjectRole::Admin)?;
        let back = format!("{}/wallets", p.base(&app));
        if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
            return Ok(err_redirect(&app, &back, &e.public_message()));
        }
        let network = app.store.network(nid).await.map_err(|_| Error::NotFound)?;
        if network.family != "evm" {
            return Err(Error::NotFound.into());
        }
        let pid = p.project.project_id;
        let Some(cold) = app.store.project_wallet(pid, nid).await?.and_then(|w| w.address).as_deref().and_then(parse_address) else {
            return Ok(err_redirect(&app, &back, "Save the cold wallet first."));
        };
        if crate::hot::address_for_project(&app, pid, "evm").await?.is_none() {
            return Ok(err_redirect(&app, &back, "This project has no EVM hot wallet. Assign one under Hot wallets first."));
        }
        let factory = evm_factory::factory_address(cold);
        let queued = app.store.queue_evm_deploy(nid, pid, &key(&factory), &cold.to_checksum(None), Some(p.user.user.user_id)).await?;
        p.audit(&app, "evm.deploy", "network", Some(&network.code), serde_json::json!({ "factory": factory.to_checksum(None) })).await;
        let message = if queued {
            format!("The worker deploys the factory on {} within a minute.", network.name)
        } else {
            format!("A deployment on {} is already on its way.", network.name)
        };
        Ok(ok_redirect(&app, &back, &message))
    }
}
