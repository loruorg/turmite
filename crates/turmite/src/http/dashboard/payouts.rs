//! Payouts in a project: list, detail, new payout, approval, address book, limits.

use std::collections::HashMap;

use askama::Template;
use axum::{
    Form, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use chrono::{Duration, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;

use super::{DashResult, Proj, ProjectRole, Shell, err_redirect, ok_redirect, page};
use crate::{
    app::App,
    db::{Page, PayoutFilter, PayoutRow, PgStore},
    error::Error,
    http::{
        checkout::fmt_amount,
        views::{ago, datetime},
    },
    payouts::{
        Catalog, PAYOUT_FAMILIES, PayoutStatus,
        limits::{PayoutLimits, Reason},
        service::{self, NewWithdrawal, Source},
    },
};

pub fn routes() -> Router<App> {
    Router::new()
        .route("/p/{pid}/payouts", get(list).post(create))
        .route("/p/{pid}/payouts/new", get(new_form))
        .route("/p/{pid}/payouts/address-book", get(address_book).post(add_address))
        .route("/p/{pid}/payouts/address-book/{aid}/delete", post(delete_address))
        .route("/p/{pid}/payouts/limits", get(limits).post(save_limits))
        .route("/p/{pid}/payouts/{id}", get(detail))
        .route("/p/{pid}/payouts/{id}/approve", post(approve))
        .route("/p/{pid}/payouts/{id}/confirm", post(confirm))
        .route("/p/{pid}/payouts/{id}/reject", post(reject))
        .route("/p/{pid}/payouts/{id}/cancel", post(cancel))
        .route("/p/{pid}/payouts/{id}/resolve", post(resolve))
}

pub fn status_class(s: PayoutStatus) -> &'static str {
    match s {
        PayoutStatus::Sent => "badge-ok",
        PayoutStatus::Failed | PayoutStatus::Rejected => "badge-bad",
        PayoutStatus::Pending | PayoutStatus::Signing | PayoutStatus::Initiated => "badge-live",
        PayoutStatus::Cancelled => "badge-plain",
        _ => "",
    }
}

pub fn status_label(s: PayoutStatus) -> &'static str {
    match s {
        PayoutStatus::PendingApproval => "Pending approval",
        PayoutStatus::PendingOtpVerification => "Pending code",
        PayoutStatus::Pending => "Pending",
        PayoutStatus::Signing => "Signing",
        PayoutStatus::Initiated => "Initiated",
        PayoutStatus::Sent => "Sent",
        PayoutStatus::Failed => "Failed",
        PayoutStatus::Rejected => "Rejected",
        PayoutStatus::Cancelled => "Cancelled",
        PayoutStatus::NeedsReview => "Needs review",
    }
}

struct Line {
    id: i64,
    href: String,
    amount: String,
    network: String,
    to: String,
    status: &'static str,
    status_label: &'static str,
    status_class: &'static str,
    created: String,
}

fn line(base: &str, p: &PayoutRow, catalog: &Catalog) -> Line {
    let an = catalog.get(p.asset_network_id).ok();
    Line {
        id: p.payout_id,
        href: format!("{base}/payouts/{}", p.payout_id),
        amount: format!("{} {}", fmt_amount(p.amount), an.map(|a| a.asset.symbol.as_str()).unwrap_or("")),
        network: an.map(|a| a.network.name.clone()).unwrap_or_default(),
        to: p.address.clone(),
        status: p.status.as_str(),
        status_label: status_label(p.status),
        status_class: status_class(p.status),
        created: ago(p.created_at),
    }
}

fn payout_networks(catalog: &Catalog) -> Vec<(i64, String)> {
    let mut v: Vec<(i64, String)> = catalog
        .all()
        .filter(|a| a.is_native() && PAYOUT_FAMILIES.contains(&a.network.family.as_str()) && a.network.is_enabled)
        .map(|a| (a.network.network_id, a.network.name.clone()))
        .collect();
    v.sort();
    v
}

// ---- list ----------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct ListQuery {
    status: Option<String>,
    network: Option<String>,
    page: Option<i64>,
}

#[derive(Template)]
#[template(path = "dashboard/payouts.html")]
struct ListPage {
    shell: Shell,
    pid: i64,
    tab: &'static str,
    lines: Vec<Line>,
    statuses: Vec<(&'static str, &'static str)>,
    networks: Vec<(i64, String)>,
    status: String,
    network: String,
    page: i64,
    pages: i64,
    total: i64,
    can_create: bool,
    waiting: i64,
}

async fn list(State(app): State<App>, p: Proj, Query(q): Query<ListQuery>) -> DashResult {
    let pid = p.project.project_id;
    let status = q.status.clone().filter(|s| PayoutStatus::parse(s).is_some());
    let network_id = q.network.as_deref().and_then(|n| n.parse::<i64>().ok());
    let filter = PayoutFilter { status: status.clone(), network_id, ..Default::default() };
    let page_no = q.page.unwrap_or(0).max(0);
    let (rows, total) = app.store.list_payouts(pid, &filter, Page::new(25, page_no)).await?;
    let catalog = Catalog::load(&app).await?;
    let base = p.base(&app);
    let waiting = app
        .store
        .payout_status_counts(pid)
        .await?
        .into_iter()
        .filter(|(s, _)| s == "pending-approval" || s == "needs-review")
        .map(|(_, n)| n)
        .sum();
    let shell = Shell::new(&app, &p.user, Some(&p.project), "payouts", "Payouts").await;
    Ok(page(
        &app,
        &ListPage {
            shell,
            pid,
            tab: "list",
            lines: rows.iter().map(|r| line(&base, r, &catalog)).collect(),
            statuses: PayoutStatus::ALL.iter().map(|s| (s.as_str(), status_label(*s))).collect(),
            networks: payout_networks(&catalog),
            status: status.unwrap_or_default(),
            network: network_id.map(|n| n.to_string()).unwrap_or_default(),
            page: page_no,
            pages: ((total + 24) / 25).max(1),
            total,
            can_create: p.role >= ProjectRole::Admin,
            waiting,
        },
    ))
}

// ---- new payout ------------------------------------------------------------------------

struct CurrencyOption {
    ticker: String,
    label: String,
}

struct BookOption {
    id: i64,
    label: String,
}

#[derive(Template)]
#[template(path = "dashboard/payout_new.html")]
struct NewPage {
    shell: Shell,
    pid: i64,
    tab: &'static str,
    currencies: Vec<CurrencyOption>,
    book: Vec<BookOption>,
    wallets: Vec<(String, String)>,
    can_create: bool,
}

fn family_label(f: &str) -> &'static str {
    match f {
        "evm" => "EVM",
        "tron" => "Tron",
        "solana" => "Solana",
        "ton" => "TON",
        _ => "",
    }
}

async fn new_form(State(app): State<App>, p: Proj) -> DashResult {
    let pid = p.project.project_id;
    let catalog = Catalog::load(&app).await?;
    let mut wallets = Vec::new();
    let mut families = Vec::new();
    for f in PAYOUT_FAMILIES {
        let w = crate::hot::address_for_project(&app, pid, f).await?;
        if w.is_some() {
            families.push(f);
        }
        wallets.push((family_label(f).to_string(), w.map(|(_, a)| a).unwrap_or_default()));
    }
    let mut currencies: Vec<CurrencyOption> = catalog
        .all()
        .filter(|a| a.is_enabled && a.network.is_enabled && families.contains(&a.network.family.as_str()))
        .map(|a| CurrencyOption { ticker: a.ticker.clone(), label: format!("{} on {} ({})", a.asset.symbol, a.network.name, a.ticker) })
        .collect();
    currencies.sort_by(|a, b| a.label.cmp(&b.label));
    let names: HashMap<i64, String> = payout_networks(&catalog).into_iter().collect();
    let book = app
        .store
        .list_payout_addresses(pid)
        .await?
        .into_iter()
        .map(|e| BookOption {
            id: e.payout_address_id,
            label: format!("{} · {} · {}", e.label, names.get(&e.network_id).cloned().unwrap_or_default(), e.address),
        })
        .collect();
    let shell = Shell::new(&app, &p.user, Some(&p.project), "payouts", "New payout").await;
    Ok(page(&app, &NewPage { shell, pid, tab: "new", currencies, book, wallets, can_create: p.role >= ProjectRole::Admin }))
}

#[derive(Deserialize)]
struct NewForm {
    currency: String,
    #[serde(default)]
    book: String,
    #[serde(default)]
    address: String,
    #[serde(default)]
    extra_id: String,
    amount: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    totp: String,
}

async fn create(State(app): State<App>, p: Proj, Form(f): Form<NewForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let pid = p.project.project_id;
    let back = format!("{}/payouts/new", p.base(&app));
    if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    let Ok(amount) = f.amount.trim().parse::<Decimal>() else {
        return Ok(err_redirect(&app, &back, "Enter the amount as a number."));
    };
    let (mut address, mut extra_id) = (f.address.trim().to_string(), f.extra_id.trim().to_string());
    if let Ok(book_id) = f.book.parse::<i64>() {
        let entry = app.store.list_payout_addresses(pid).await?.into_iter().find(|e| e.payout_address_id == book_id);
        let Some(entry) = entry else { return Ok(err_redirect(&app, &back, "That address-book entry does not exist.")) };
        let an = app.store.asset_network_by_ticker(&f.currency).await?;
        if an.network.network_id != entry.network_id {
            return Ok(err_redirect(&app, &back, "The address-book entry is on another network than the currency."));
        }
        address = entry.address;
        extra_id = entry.extra_id.unwrap_or_default();
    }
    if address.is_empty() {
        return Ok(err_redirect(&app, &back, "Enter an address or choose one from the address book."));
    }
    let w = NewWithdrawal {
        address,
        currency: f.currency.clone(),
        amount,
        extra_id: Some(extra_id),
        description: Some(f.description),
        ..Default::default()
    };
    match service::create_batch(&app, pid, Source::Dashboard { user_id: p.user.user.user_id }, vec![w], None).await {
        Ok(out) => {
            let payout = &out.payouts[0];
            p.audit(
                &app,
                "payout.create",
                "payout",
                Some(&payout.payout_id.to_string()),
                serde_json::json!({ "currency": f.currency, "amount": payout.amount.to_string(), "address": payout.address, "status": payout.status.as_str() }),
            )
            .await;
            let msg = match payout.status {
                PayoutStatus::Pending => "Payout created. It goes out in a few seconds.",
                _ => "Payout created. It waits for an approval.",
            };
            Ok(ok_redirect(&app, &format!("{}/payouts/{}", p.base(&app), payout.payout_id), msg))
        }
        Err(e) => Ok(err_redirect(&app, &back, &e.public_message())),
    }
}

// ---- detail ------------------------------------------------------------------------------

struct AttemptLine {
    txid: String,
    href: String,
    nonce: String,
    state: String,
    created: String,
}

#[derive(Template)]
#[template(path = "dashboard/payout.html")]
struct DetailPage {
    shell: Shell,
    pid: i64,
    id: i64,
    status: &'static str,
    status_label: &'static str,
    status_class: &'static str,
    fields: Vec<(&'static str, String)>,
    tx_href: String,
    address_href: String,
    attempts: Vec<AttemptLine>,
    error: String,
    can_approve: bool,
    can_reject: bool,
    can_cancel: bool,
    can_resolve: bool,
    awaits_api: bool,
    code_mine: bool,
    code_other: String,
    batch_id: i64,
}

async fn email_of(app: &App, user_id: Option<i64>) -> String {
    match user_id {
        Some(id) => app.store.user(id).await.map(|u| u.email).unwrap_or_else(|_| format!("user {id}")),
        None => String::new(),
    }
}

async fn detail(State(app): State<App>, p: Proj, Path((_pid, id)): Path<(i64, i64)>) -> DashResult {
    let pid = p.project.project_id;
    let po = app.store.payout(pid, id).await?.ok_or(Error::NotFound)?;
    let catalog = Catalog::load(&app).await?;
    let an = catalog.get(po.asset_network_id)?;
    let net = &an.network;
    let native = catalog.native_ticker(net.network_id).to_ascii_uppercase();
    let mut fields = vec![
        ("Amount", format!("{} {}", fmt_amount(po.amount), an.asset.symbol)),
        ("Network", format!("{} ({})", net.name, an.ticker)),
        ("To", po.address.clone()),
    ];
    if let Some(m) = &po.extra_id {
        fields.push(("Memo", m.clone()));
    }
    if let Some(u) = po.amount_usd {
        fields.push(("Value at creation", format!("{} USD", u.round_dp(2))));
    }
    if let Some(d) = &po.description {
        fields.push(("Description", d.clone()));
    }
    if let Some(k) = &po.unique_external_id {
        fields.push(("External id", k.clone()));
    }
    fields.push(("Batch", po.batch_id.to_string()));
    if let Some(r) = &po.approval_reason {
        fields.push(("Needs approval", Reason::text(r).to_string()));
    }
    if let Some(f) = &po.from_address {
        fields.push(("From hot wallet", f.clone()));
    }
    if let Some(t) = &po.txid {
        fields.push(("Transaction", t.clone()));
    }
    if matches!(po.status, PayoutStatus::Initiated | PayoutStatus::Sent) {
        fields.push((
            "Confirmations",
            format!("{} of {}", po.confirmations.min(net.confirmations_required.max(1)), net.confirmations_required.max(1)),
        ));
    }
    if let Some(fee) = po.fee {
        fields.push(("Network fee", format!("{} {native}", fmt_amount(fee))));
    }
    fields.push(("Created", format!("{} by {}", datetime(po.created_at), email_of(&app, po.created_by).await)));
    if let Some(t) = po.verified_at {
        fields.push(("Verified", format!("{} by {}", datetime(t), email_of(&app, po.verified_by).await)));
    }
    if let (Some(t), Some(_)) = (po.approved_at, po.approved_by) {
        fields.push(("Approved", format!("{} by {}", datetime(t), email_of(&app, po.approved_by).await)));
    }
    if let Some(t) = po.requested_at {
        fields.push(("Broadcast", datetime(t)));
    }
    if let Some(u) = &po.ipn_callback_url {
        fields.push(("IPN URL", u.clone()));
    }
    if let Some(n) = &po.resolution_note {
        fields.push(("Review note", n.clone()));
    }
    let attempts = app
        .store
        .payout_attempts(id)
        .await?
        .into_iter()
        .rev()
        .map(|a| AttemptLine {
            href: net.tx_url(&a.txid).unwrap_or_default(),
            nonce: a.nonce.map(|n| n.to_string()).unwrap_or_default(),
            state: match (&a.broadcast_at, &a.last_error) {
                (Some(_), _) => "Accepted by a node".into(),
                (None, Some(e)) => format!("Refused: {e}"),
                (None, None) => "Stored".into(),
            },
            created: ago(a.created_at),
            txid: a.txid,
        })
        .collect();
    let code = app.store.approval_code(id).await?;
    let me = p.user.user.user_id;
    let awaits_api = po.status == PayoutStatus::PendingOtpVerification && po.verified_at.is_none();
    let code_mine =
        po.status == PayoutStatus::PendingOtpVerification && code.as_ref().is_some_and(|c| c.user_id == me && c.expires_at > Utc::now());
    let code_other = match &code {
        Some(c) if po.status == PayoutStatus::PendingOtpVerification && c.user_id != me => email_of(&app, Some(c.user_id)).await,
        _ => String::new(),
    };
    let can_act = p.role >= ProjectRole::Admin;
    let shell = Shell::new(&app, &p.user, Some(&p.project), "payouts", &format!("Payout {id}")).await;
    Ok(page(
        &app,
        &DetailPage {
            shell,
            pid,
            id,
            status: po.status.as_str(),
            status_label: status_label(po.status),
            status_class: status_class(po.status),
            tx_href: po.txid.as_deref().and_then(|t| net.tx_url(t)).unwrap_or_default(),
            address_href: net.address_url(&po.address).unwrap_or_default(),
            fields,
            attempts,
            error: po.error.clone().unwrap_or_default(),
            can_approve: can_act
                && (po.status == PayoutStatus::PendingApproval || (po.status == PayoutStatus::PendingOtpVerification && !awaits_api)),
            can_reject: can_act && PayoutStatus::REJECTABLE.contains(&po.status),
            can_cancel: can_act && PayoutStatus::CANCELLABLE.contains(&po.status),
            can_resolve: can_act && po.status == PayoutStatus::NeedsReview,
            awaits_api,
            code_mine,
            code_other,
            batch_id: po.batch_id,
        },
    ))
}

// ---- actions -----------------------------------------------------------------------------

#[derive(Deserialize)]
struct TotpForm {
    #[serde(default)]
    totp: String,
}

async fn approve(State(app): State<App>, p: Proj, Path((_pid, id)): Path<(i64, i64)>, Form(f): Form<TotpForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/payouts/{id}", p.base(&app));
    if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    match service::start_approval(&app, p.project.project_id, id, &p.user.user).await {
        Ok(_) => {
            p.audit(&app, "payout.approve_start", "payout", Some(&id.to_string()), serde_json::json!({})).await;
            Ok(ok_redirect(&app, &back, &format!("A code was sent to {}. Enter it below within 10 minutes.", p.user.user.email)))
        }
        Err(e) => Ok(err_redirect(&app, &back, &e.public_message())),
    }
}

#[derive(Deserialize)]
struct CodeForm {
    #[serde(default)]
    code: String,
}

async fn confirm(State(app): State<App>, p: Proj, Path((_pid, id)): Path<(i64, i64)>, Form(f): Form<CodeForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/payouts/{id}", p.base(&app));
    match service::confirm_approval(&app, p.project.project_id, id, p.user.user.user_id, &f.code).await {
        Ok(_) => {
            p.audit(&app, "payout.approve", "payout", Some(&id.to_string()), serde_json::json!({})).await;
            Ok(ok_redirect(&app, &back, "Payout approved. It goes out in a few seconds."))
        }
        Err(e) => {
            p.audit(&app, "payout.approve_failed", "payout", Some(&id.to_string()), serde_json::json!({ "error": e.public_message() }))
                .await;
            Ok(err_redirect(&app, &back, &e.public_message()))
        }
    }
}

async fn reject(State(app): State<App>, p: Proj, Path((_pid, id)): Path<(i64, i64)>, Form(f): Form<TotpForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/payouts/{id}", p.base(&app));
    if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    match service::reject(&app, p.project.project_id, id).await {
        Ok(_) => {
            p.audit(&app, "payout.reject", "payout", Some(&id.to_string()), serde_json::json!({})).await;
            Ok(ok_redirect(&app, &back, "Payout rejected."))
        }
        Err(e) => Ok(err_redirect(&app, &back, &e.public_message())),
    }
}

async fn cancel(State(app): State<App>, p: Proj, Path((_pid, id)): Path<(i64, i64)>, Form(f): Form<TotpForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/payouts/{id}", p.base(&app));
    if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    match service::cancel(&app, p.project.project_id, id).await {
        Ok(_) => {
            p.audit(&app, "payout.cancel", "payout", Some(&id.to_string()), serde_json::json!({})).await;
            Ok(ok_redirect(&app, &back, "Payout cancelled."))
        }
        Err(e) => Ok(err_redirect(&app, &back, &e.public_message())),
    }
}

#[derive(Deserialize)]
struct ResolveForm {
    #[serde(default)]
    outcome: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    totp: String,
}

async fn resolve(State(app): State<App>, p: Proj, Path((_pid, id)): Path<(i64, i64)>, Form(f): Form<ResolveForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/payouts/{id}", p.base(&app));
    if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    let sent = match f.outcome.as_str() {
        "sent" => true,
        "not_sent" => false,
        _ => return Ok(err_redirect(&app, &back, "Choose whether the payout was sent.")),
    };
    match service::resolve(&app, p.project.project_id, id, sent, &f.note).await {
        Ok(_) => {
            p.audit(&app, "payout.resolve", "payout", Some(&id.to_string()), serde_json::json!({ "sent": sent, "note": f.note.trim() }))
                .await;
            Ok(ok_redirect(&app, &back, if sent { "Marked as sent." } else { "Marked as not sent." }))
        }
        Err(e) => Ok(err_redirect(&app, &back, &e.public_message())),
    }
}

// ---- address book --------------------------------------------------------------------------

struct BookLine {
    id: i64,
    label: String,
    network: String,
    address: String,
    memo: String,
    added: String,
}

#[derive(Template)]
#[template(path = "dashboard/payout_addresses.html")]
struct BookPage {
    shell: Shell,
    pid: i64,
    tab: &'static str,
    entries: Vec<BookLine>,
    networks: Vec<(i64, String)>,
    can_edit: bool,
}

async fn address_book(State(app): State<App>, p: Proj) -> DashResult {
    let pid = p.project.project_id;
    let catalog = Catalog::load(&app).await?;
    let networks = payout_networks(&catalog);
    let names: HashMap<i64, String> = app.store.list_networks().await?.into_iter().map(|n| (n.network_id, n.name)).collect();
    let entries = app
        .store
        .list_payout_addresses(pid)
        .await?
        .into_iter()
        .map(|e| BookLine {
            id: e.payout_address_id,
            label: e.label,
            network: names.get(&e.network_id).cloned().unwrap_or_default(),
            address: e.address,
            memo: e.extra_id.unwrap_or_default(),
            added: ago(e.created_at),
        })
        .collect();
    let shell = Shell::new(&app, &p.user, Some(&p.project), "payouts", "Address book").await;
    Ok(page(&app, &BookPage { shell, pid, tab: "book", entries, networks, can_edit: p.role >= ProjectRole::Admin }))
}

#[derive(Deserialize)]
struct AddressForm {
    network: i64,
    label: String,
    address: String,
    #[serde(default)]
    extra_id: String,
    #[serde(default)]
    totp: String,
}

async fn add_address(State(app): State<App>, p: Proj, Form(f): Form<AddressForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/payouts/address-book", p.base(&app));
    if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    let memo = Some(f.extra_id.trim()).filter(|m| !m.is_empty());
    match service::add_address(&app, p.project.project_id, f.network, &f.label, &f.address, memo, p.user.user.user_id).await {
        Ok((id, address)) => {
            p.audit(
                &app,
                "payout_address.add",
                "payout_address",
                Some(&id.to_string()),
                serde_json::json!({ "network_id": f.network, "label": f.label.trim(), "address": address, "memo": memo }),
            )
            .await;
            Ok(ok_redirect(&app, &back, "Address added."))
        }
        Err(e) => Ok(err_redirect(&app, &back, &e.public_message())),
    }
}

async fn delete_address(State(app): State<App>, p: Proj, Path((_pid, aid)): Path<(i64, i64)>, Form(f): Form<TotpForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/payouts/address-book", p.base(&app));
    if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    match app.store.delete_payout_address(p.project.project_id, aid).await? {
        Some(e) => {
            p.audit(
                &app,
                "payout_address.delete",
                "payout_address",
                Some(&aid.to_string()),
                serde_json::json!({ "address": e.address, "label": e.label }),
            )
            .await;
            Ok(ok_redirect(&app, &back, "Address removed."))
        }
        None => Ok(err_redirect(&app, &back, "That entry does not exist.")),
    }
}

// ---- limits ----------------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "dashboard/payout_limits.html")]
struct LimitsPage {
    shell: Shell,
    pid: i64,
    tab: &'static str,
    min_usd: String,
    auto_approve_usd: String,
    hourly_usd: String,
    daily_usd: String,
    used_hour: String,
    used_day: String,
    can_edit: bool,
}

fn money(d: Decimal) -> String {
    d.round_dp(2).normalize().to_string()
}

async fn limits(State(app): State<App>, p: Proj) -> DashResult {
    let pid = p.project.project_id;
    let l = app.store.payout_limits(pid).await?;
    let mut conn = app.store.pool.acquire().await?;
    let used_hour = PgStore::payout_usage_usd(&mut conn, pid, Utc::now() - Duration::hours(1)).await?;
    let used_day = PgStore::payout_usage_usd(&mut conn, pid, Utc::now() - Duration::hours(24)).await?;
    drop(conn);
    let shell = Shell::new(&app, &p.user, Some(&p.project), "payouts", "Payout limits").await;
    Ok(page(
        &app,
        &LimitsPage {
            shell,
            pid,
            tab: "limits",
            min_usd: money(l.min_usd),
            auto_approve_usd: money(l.auto_approve_usd),
            hourly_usd: money(l.hourly_usd),
            daily_usd: money(l.daily_usd),
            used_hour: money(used_hour),
            used_day: money(used_day),
            can_edit: p.role >= ProjectRole::Admin,
        },
    ))
}

#[derive(Deserialize)]
struct LimitsForm {
    min_usd: String,
    auto_approve_usd: String,
    hourly_usd: String,
    daily_usd: String,
    #[serde(default)]
    totp: String,
}

async fn save_limits(State(app): State<App>, p: Proj, Form(f): Form<LimitsForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/payouts/limits", p.base(&app));
    if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    let parse = |s: &str| -> Option<Decimal> {
        let s = s.trim();
        if s.is_empty() { Some(Decimal::ZERO) } else { s.parse::<Decimal>().ok().map(|d| d.round_dp(2)) }
    };
    let (Some(min_usd), Some(auto_approve_usd), Some(hourly_usd), Some(daily_usd)) =
        (parse(&f.min_usd), parse(&f.auto_approve_usd), parse(&f.hourly_usd), parse(&f.daily_usd))
    else {
        return Ok(err_redirect(&app, &back, "Enter each amount as a number."));
    };
    let l = PayoutLimits { min_usd, auto_approve_usd, hourly_usd, daily_usd };
    if let Err(e) = l.validate() {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    app.store.set_payout_limits(p.project.project_id, &l).await?;
    p.audit(
        &app,
        "payout_limits.save",
        "project",
        Some(&p.project.project_id.to_string()),
        serde_json::json!({ "min_usd": money(min_usd), "auto_approve_usd": money(auto_approve_usd), "hourly_usd": money(hourly_usd), "daily_usd": money(daily_usd) }),
    )
    .await;
    Ok(ok_redirect(&app, &back, "Limits saved."))
}
