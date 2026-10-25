//! Server > Hot wallets: generate or import keys, see balances, assign wallets to projects. Admins only, and
//! every change takes a fresh two-factor code. A key is never shown or exported.

use std::time::Duration;

use askama::Template;
use axum::{Form, Router, extract::State, routing::get, routing::post};
use serde::Deserialize;
use zeroize::Zeroizing;

use super::{DashResult, Shell, User, err_redirect, ok_redirect, page};
use crate::{
    app::App,
    error::Error,
    hot,
    http::{checkout::fmt_amount, views::ago},
    payouts::{PAYOUT_FAMILIES, family_has_payouts, service},
};

pub fn routes() -> Router<App> {
    Router::new()
        .route("/admin/hot-wallets", get(index))
        .route("/admin/hot-wallets/generate", post(generate))
        .route("/admin/hot-wallets/import", post(import))
        .route("/admin/hot-wallets/assign", post(assign))
}

pub fn family_name(f: &str) -> &'static str {
    match f {
        "evm" => "EVM (Ethereum, Base, Polygon, BNB Smart Chain)",
        "tron" => "Tron",
        "solana" => "Solana",
        "ton" => "TON",
        _ => "",
    }
}

struct Balance {
    network: String,
    amount: String,
}

struct WalletCard {
    label: String,
    family: String,
    family_name: &'static str,
    address: String,
    created: String,
    balances: Vec<Balance>,
    projects: Vec<String>,
}

struct Assignment {
    project: String,
    family: &'static str,
    wallet: String,
}

#[derive(Template)]
#[template(path = "dashboard/hot_wallets.html")]
struct HotWalletsPage {
    shell: Shell,
    wallets: Vec<WalletCard>,
    families: Vec<(&'static str, &'static str)>,
    projects: Vec<(i64, String)>,
    options: Vec<(i64, String)>,
    assignments: Vec<Assignment>,
    can_store: bool,
}

async fn index(State(app): State<App>, user: User) -> DashResult {
    user.require_admin()?;
    let wallets = hot::list(&app).await?;
    let networks = app.store.list_networks().await?;
    let nodes = app.store.list_nodes(None).await?;
    let projects: Vec<(i64, String)> = app.store.list_projects().await?.into_iter().map(|p| (p.project_id, p.name)).collect();
    let assigned = hot::assignments(&app).await?;
    let project_name = |id: i64| projects.iter().find(|(p, _)| *p == id).map(|(_, n)| n.clone()).unwrap_or_default();

    // Balances: every enabled network of the wallet's family that has a node, at most 5 s each, in parallel.
    let mut jobs = tokio::task::JoinSet::new();
    for w in &wallets {
        for n in networks
            .iter()
            .filter(|n| n.family == w.family && n.is_enabled && nodes.iter().any(|x| x.network_id == n.network_id && x.is_enabled))
        {
            let (app, n, address, id) = (app.clone(), n.clone(), w.address.clone(), w.hot_wallet_id);
            jobs.spawn(async move {
                let r = tokio::time::timeout(Duration::from_secs(5), service::native_balance(&app, &n, &address)).await;
                let amount = match r {
                    Ok(Ok(b)) => format!("{} {}", fmt_amount(b), n.code),
                    Ok(Err(_)) | Err(_) => "No answer from the node".to_string(),
                };
                (id, n.network_id, Balance { network: n.name.clone(), amount })
            });
        }
    }
    let mut balances: Vec<(i64, i64, Balance)> = Vec::new();
    while let Some(Ok(b)) = jobs.join_next().await {
        balances.push(b);
    }
    balances.sort_by_key(|(id, nid, _)| (*id, *nid));

    let cards = wallets
        .iter()
        .map(|w| WalletCard {
            label: w.label.clone(),
            family: w.family.clone(),
            family_name: family_name(&w.family),
            address: w.address.clone(),
            created: ago(w.created_at),
            balances: balances
                .iter()
                .filter(|(id, _, _)| *id == w.hot_wallet_id)
                .map(|(_, _, b)| Balance { network: b.network.clone(), amount: b.amount.clone() })
                .collect(),
            projects: assigned.iter().filter(|(_, _, h)| *h == w.hot_wallet_id).map(|(p, _, _)| project_name(*p)).collect(),
        })
        .collect();
    let options = wallets.iter().map(|w| (w.hot_wallet_id, format!("{} · {} · {}", w.label, w.family, w.address))).collect();
    let assignments = assigned
        .iter()
        .map(|(p, f, h)| Assignment {
            project: project_name(*p),
            family: family_name(f),
            wallet: wallets.iter().find(|w| w.hot_wallet_id == *h).map(|w| format!("{} · {}", w.label, w.address)).unwrap_or_default(),
        })
        .collect();
    let shell = Shell::new(&app, &user, None, "hot-wallets", "Hot wallets").await;
    Ok(page(
        &app,
        &HotWalletsPage {
            shell,
            wallets: cards,
            families: PAYOUT_FAMILIES.iter().map(|f| (*f, family_name(f))).collect(),
            projects,
            options,
            assignments,
            can_store: app.hot_recipient().is_some(),
        },
    ))
}

fn back(app: &App) -> String {
    app.path("/dashboard/admin/hot-wallets")
}

fn check_label(label: &str) -> Result<String, Error> {
    let l = label.trim();
    if l.is_empty() || l.chars().count() > 80 {
        return Err(Error::BadRequest("Give a label of 1 to 80 characters.".into()));
    }
    Ok(l.to_string())
}

#[derive(Deserialize)]
struct GenerateForm {
    family: String,
    label: String,
    #[serde(default)]
    totp: String,
}

async fn generate(State(app): State<App>, user: User, Form(f): Form<GenerateForm>) -> DashResult {
    user.require_admin()?;
    if let Err(e) = user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back(&app), &e.public_message()));
    }
    let result = async {
        if !family_has_payouts(&f.family) {
            return Err(Error::BadRequest("Choose EVM, Tron, Solana or TON.".into()));
        }
        let label = check_label(&f.label)?;
        let secret = hot::generate_secret(&f.family)?;
        let address = hot::derive_address(&f.family, &secret, !app.config.mode.is_mainnet())?;
        let id = hot::store(&app, &f.family, &label, &address, &secret, Some(user.user.user_id)).await?;
        Ok((id, address))
    }
    .await;
    match result {
        Ok((id, address)) => {
            user.audit(
                &app,
                None,
                "hot_wallet.generate",
                "hot_wallet",
                Some(&id.to_string()),
                serde_json::json!({ "family": f.family, "address": address }),
            )
            .await;
            Ok(ok_redirect(&app, &back(&app), &format!("Hot wallet made: {address}. Fund it, then assign it to projects.")))
        }
        Err(e) => Ok(err_redirect(&app, &back(&app), &e.public_message())),
    }
}

#[derive(Deserialize)]
struct ImportForm {
    family: String,
    label: String,
    secret: String,
    #[serde(default)]
    totp: String,
}

async fn import(State(app): State<App>, user: User, Form(mut f): Form<ImportForm>) -> DashResult {
    let input = Zeroizing::new(std::mem::take(&mut f.secret));
    user.require_admin()?;
    if let Err(e) = user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back(&app), &e.public_message()));
    }
    let result = async {
        if !family_has_payouts(&f.family) {
            return Err(Error::BadRequest("Choose EVM, Tron, Solana or TON.".into()));
        }
        let label = check_label(&f.label)?;
        let secret = hot::parse_secret(&f.family, &input)?;
        let address = hot::derive_address(&f.family, &secret, !app.config.mode.is_mainnet())?;
        let id = hot::store(&app, &f.family, &label, &address, &secret, Some(user.user.user_id)).await?;
        Ok((id, address))
    }
    .await;
    match result {
        Ok((id, address)) => {
            user.audit(
                &app,
                None,
                "hot_wallet.import",
                "hot_wallet",
                Some(&id.to_string()),
                serde_json::json!({ "family": f.family, "address": address }),
            )
            .await;
            Ok(ok_redirect(&app, &back(&app), &format!("Hot wallet imported: {address}.")))
        }
        Err(e) => Ok(err_redirect(&app, &back(&app), &e.public_message())),
    }
}

#[derive(Deserialize)]
struct AssignForm {
    project: i64,
    family: String,
    #[serde(default)]
    wallet: String,
    #[serde(default)]
    totp: String,
}

async fn assign(State(app): State<App>, user: User, Form(f): Form<AssignForm>) -> DashResult {
    user.require_admin()?;
    if let Err(e) = user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back(&app), &e.public_message()));
    }
    if !family_has_payouts(&f.family) {
        return Ok(err_redirect(&app, &back(&app), "Choose EVM, Tron, Solana or TON."));
    }
    let project = app.store.project(f.project).await?;
    match f.wallet.parse::<i64>() {
        Ok(wallet_id) => {
            let w = hot::get(&app, wallet_id).await?;
            if w.family != f.family {
                return Ok(err_redirect(&app, &back(&app), &format!("That wallet is a {} wallet.", family_name(&w.family))));
            }
            hot::assign(&app, project.project_id, &f.family, wallet_id).await?;
            user.audit(
                &app,
                Some(project.project_id),
                "hot_wallet.assign",
                "hot_wallet",
                Some(&wallet_id.to_string()),
                serde_json::json!({ "family": f.family, "address": w.address }),
            )
            .await;
            Ok(ok_redirect(&app, &back(&app), &format!("{} now uses {} for {}.", project.name, w.label, family_name(&f.family))))
        }
        Err(_) => {
            hot::unassign(&app, project.project_id, &f.family).await?;
            user.audit(
                &app,
                Some(project.project_id),
                "hot_wallet.unassign",
                "project",
                Some(&project.project_id.to_string()),
                serde_json::json!({ "family": f.family }),
            )
            .await;
            Ok(ok_redirect(&app, &back(&app), &format!("{} has no {} hot wallet now.", project.name, family_name(&f.family))))
        }
    }
}
