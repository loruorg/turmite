use askama::Template;
use axum::{
    Form, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use rust_decimal::Decimal;
use serde::Deserialize;

use super::{DashResult, Proj, ProjectRole, Shell, err_redirect, ok_redirect, page};
use crate::{
    app::App,
    chains::driver::Observation,
    db::{Page, PaymentFilter},
    error::Error,
    http::{
        checkout::{fmt_amount, fmt_fiat},
        views::{PaymentLine, ago, all_statuses, datetime, payment_lines, status_class, status_label, stuck_lines},
    },
    payments::{self, CreateInvoice},
};

pub fn routes() -> Router<App> {
    Router::new()
        .route("/p/{pid}/payments", get(list))
        .route("/p/{pid}/payments/{id}", get(detail))
        .route("/p/{pid}/payments/{id}/mark-paid", post(mark_paid))
        .route("/p/{pid}/payments/{id}/refunded", post(mark_refunded))
        .route("/p/{pid}/payments/{id}/simulate", post(simulate))
        .route("/p/{pid}/invoices", get(invoices).post(create_invoice))
}

#[derive(Deserialize, Default)]
struct ListQuery {
    status: Option<String>,
    order: Option<String>,
    page: Option<i64>,
}

#[derive(Template)]
#[template(path = "dashboard/payments.html")]
struct ListPage {
    shell: Shell,
    pid: i64,
    recent: Vec<PaymentLine>,
    statuses: Vec<(&'static str, String)>,
    status: String,
    order: String,
    page: i64,
    pages: i64,
    total: i64,
}

async fn list(State(app): State<App>, p: Proj, Query(q): Query<ListQuery>) -> DashResult {
    let filter = PaymentFilter {
        status: q.status.clone().filter(|s| !s.is_empty()),
        order_id: q.order.clone().filter(|s| !s.is_empty()),
        ..Default::default()
    };
    let page_no = q.page.unwrap_or(0).max(0);
    let (rows, total) = app.store.list_payments(p.project.project_id, &filter, Page::new(25, page_no)).await?;
    let recent = payment_lines(&app, &rows, &p.base(&app)).await?;
    let shell = Shell::new(&app, &p.user, Some(&p.project), "payments", "Payments").await;
    Ok(page(
        &app,
        &ListPage {
            shell,
            pid: p.project.project_id,
            recent,
            statuses: all_statuses(),
            status: q.status.unwrap_or_default(),
            order: q.order.unwrap_or_default(),
            page: page_no,
            pages: ((total + 24) / 25).max(1),
            total,
        },
    ))
}

struct TxLine {
    txid: String,
    href: String,
    amount: String,
    /// Count, or "Dropped" for a transaction that left the mempool without a block.
    confirmations: String,
    seen: String,
}

struct SweepLine {
    tx: TxLine,
    /// "Sent", "Confirmed", or "Failed: <reason>".
    status: String,
}

#[derive(Template)]
#[template(path = "dashboard/payment.html")]
struct DetailPage {
    shell: Shell,
    pid: i64,
    id: i64,
    status: String,
    status_label: String,
    status_class: &'static str,
    fields: Vec<(&'static str, String)>,
    address_href: String,
    checkout_url: String,
    txs: Vec<TxLine>,
    sweeps: Vec<SweepLine>,
    /// Why the funds still wait at the deposit address, when they wait too long.
    stuck: String,
    can_act: bool,
    can_refund: bool,
    demo: bool,
    pay_amount: String,
}

async fn detail(State(app): State<App>, p: Proj, Path((_pid, id)): Path<(i64, i64)>) -> DashResult {
    let pay = app.store.payment(p.project.project_id, id).await?.ok_or(Error::NotFound)?;
    let an = app.store.asset_network(pay.asset_network_id).await?;
    let net = &an.network;
    let txs = app.store.payment_transactions(id).await?;
    let sweeps = app.store.sweeps_for_payment(id).await?;
    let mut fields = vec![
        ("Order", pay.order_id.clone().unwrap_or_else(|| "–".into())),
        ("Description", pay.order_description.clone().unwrap_or_else(|| "–".into())),
        ("Price", fmt_fiat(pay.price_amount, &pay.price_currency)),
        ("Amount due", format!("{} {}", fmt_amount(pay.pay_amount), an.asset.symbol)),
        ("Received", format!("{} {}", fmt_amount(pay.actually_paid), an.asset.symbol)),
        ("Credited", pay.outcome_amount.map(|d| format!("{} {}", fmt_amount(d), an.asset.symbol)).unwrap_or_else(|| "–".into())),
        ("Currency", format!("{} on {}", an.asset.symbol, net.name)),
        ("Address", pay.pay_address.clone()),
    ];
    if let Some(m) = &pay.payin_extra_id {
        fields.push(("Memo", m.clone()));
    }
    if let Some(r) = pay.rate {
        fields.push(("Rate", format!("1 {} = {} {}", an.asset.symbol, r.normalize(), pay.price_currency.to_ascii_uppercase())));
    }
    fields.push(("Settlement", status_label(pay.settlement.as_str())));
    fields.push(("Created", datetime(pay.created_at)));
    fields.push(("Expires", datetime(pay.expires_at)));
    if let Some(t) = pay.paid_at {
        fields.push(("Paid", datetime(t)));
    }
    if let Some(u) = &pay.ipn_callback_url {
        fields.push(("IPN URL", u.clone()));
    }
    let line = |txid: &str, amount: Decimal, conf: i32, at| TxLine {
        txid: txid.to_string(),
        href: net.tx_url(txid).unwrap_or_default(),
        amount: format!("{} {}", fmt_amount(amount), an.asset.symbol),
        confirmations: if conf < 0 { "Dropped".into() } else { conf.to_string() },
        seen: ago(at),
    };
    let domain = app.store.checkout_domain(p.project.project_id).await?;
    let shell = Shell::new(&app, &p.user, Some(&p.project), "payments", &format!("Payment {id}")).await;
    Ok(page(
        &app,
        &DetailPage {
            shell,
            pid: p.project.project_id,
            id,
            status: pay.status.as_str().to_string(),
            status_label: status_label(pay.status.as_str()),
            status_class: status_class(pay.status.as_str()),
            fields,
            address_href: net.address_url(&pay.pay_address).unwrap_or_default(),
            checkout_url: crate::http::payment_url(&app, domain.as_deref(), &pay.token),
            txs: txs.iter().map(|t| line(&t.txid, t.amount, t.confirmations, t.seen_at)).collect(),
            // Bitcoin and Monero deposits land in the merchant's wallet: nothing is swept.
            sweeps: if matches!(net.family.as_str(), "bitcoin" | "monero") {
                vec![]
            } else {
                sweeps
                    .iter()
                    .map(|s| SweepLine {
                        tx: line(s.txid.as_deref().unwrap_or("pending"), s.amount, 0, s.created_at),
                        status: match (s.status.as_str(), &s.last_error) {
                            ("broadcast", _) => "Sent".into(),
                            ("failed", Some(e)) => format!("Failed: {e}"),
                            (other, _) => status_label(other),
                        },
                    })
                    .collect()
            },
            stuck: stuck_lines(&app, p.project.project_id, &p.base(&app))
                .await?
                .into_iter()
                .find(|s| s.id == id)
                .map(|s| s.text)
                .unwrap_or_default(),
            can_act: p.role >= ProjectRole::Admin,
            can_refund: pay.status.can_refund(pay.actually_paid),
            demo: app.demo,
            pay_amount: fmt_amount(pay.pay_amount),
        },
    ))
}

#[derive(Deserialize)]
struct TotpForm {
    #[serde(default)]
    totp: String,
}

async fn mark_paid(State(app): State<App>, p: Proj, Path((_pid, id)): Path<(i64, i64)>, Form(f): Form<TotpForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/payments/{id}", p.base(&app));
    if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    if app.store.mark_paid_manually(p.project.project_id, id).await? {
        let pay = app.store.payment(p.project.project_id, id).await?.ok_or(Error::NotFound)?;
        crate::webhooks::payment_changed(&app, &pay).await?;
        p.audit(&app, "payment.mark_paid", "payment", Some(&id.to_string()), serde_json::json!({})).await;
    }
    Ok(ok_redirect(&app, &back, "Payment marked as paid."))
}

async fn mark_refunded(State(app): State<App>, p: Proj, Path((_pid, id)): Path<(i64, i64)>, Form(f): Form<TotpForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/payments/{id}", p.base(&app));
    if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    if !app.store.mark_refunded(p.project.project_id, id).await? {
        return Ok(err_redirect(&app, &back, "You can mark a payment as refunded only when the customer sent funds."));
    }
    let pay = app.store.payment(p.project.project_id, id).await?.ok_or(Error::NotFound)?;
    crate::webhooks::payment_changed(&app, &pay).await?;
    p.audit(&app, "payment.mark_refunded", "payment", Some(&id.to_string()), serde_json::json!({})).await;
    Ok(ok_redirect(&app, &back, "Payment marked as refunded."))
}

#[derive(Deserialize)]
struct SimulateForm {
    amount: String,
    confirmations: i32,
}

/// Demo mode only: pretend a deposit arrived.
async fn simulate(State(app): State<App>, p: Proj, Path((_pid, id)): Path<(i64, i64)>, Form(f): Form<SimulateForm>) -> DashResult {
    p.require(ProjectRole::Developer)?;
    let back = format!("{}/payments/{id}", p.base(&app));
    if !app.demo {
        return Err(Error::NotFound.into());
    }
    let pay = app.store.payment(p.project.project_id, id).await?.ok_or(Error::NotFound)?;
    let amount: Decimal = f.amount.trim().parse().map_err(|_| Error::BadRequest("Enter an amount.".into()))?;
    let net = app.store.network(pay.network_id).await?;
    if let Some(driver) = app.drivers().get(&net.chain)
        && let Some(mock) = crate::chains::driver::as_mock(&driver)
    {
        let n = app.store.payment_transactions(id).await?.len();
        let txid = format!("demo{}{}", id, n + 1);
        mock.inject(
            &pay.pay_address,
            Observation { chain_tx_key: txid.clone(), txid, vout: None, amount, confirmations: f.confirmations.max(0), block_height: None },
        )
        .await;
    }
    crate::worker::process_payment(&app, id).await?;
    Ok(ok_redirect(&app, &back, "Deposit simulated."))
}

// ---- invoices ------------------------------------------------------------------

struct InvoiceLine {
    id: i64,
    url: String,
    order: String,
    price: String,
    created: String,
    expired: bool,
}

#[derive(Template)]
#[template(path = "dashboard/invoices.html")]
struct InvoicesPage {
    shell: Shell,
    pid: i64,
    rows: Vec<InvoiceLine>,
    new_url: String,
    can_create: bool,
}

async fn invoices_view(app: &App, p: &Proj, new_url: String) -> DashResult {
    let domain = app.store.checkout_domain(p.project.project_id).await?;
    let rows = app
        .store
        .list_invoices(p.project.project_id, Page::new(50, 0))
        .await?
        .into_iter()
        .map(|i| InvoiceLine {
            id: i.invoice_id,
            url: crate::http::invoice_url(app, domain.as_deref(), &i.token),
            order: i.order_description.clone().or(i.order_id.clone()).unwrap_or_default(),
            price: fmt_fiat(i.price_amount, &i.price_currency),
            created: ago(i.created_at),
            expired: i.expires_at < chrono::Utc::now(),
        })
        .collect();
    let shell = Shell::new(app, &p.user, Some(&p.project), "invoices", "Invoices").await;
    Ok(page(app, &InvoicesPage { shell, pid: p.project.project_id, rows, new_url, can_create: p.role >= ProjectRole::Developer }))
}

async fn invoices(State(app): State<App>, p: Proj) -> DashResult {
    invoices_view(&app, &p, String::new()).await
}

#[derive(Deserialize)]
struct InvoiceForm {
    amount: String,
    currency: String,
    description: String,
}

async fn create_invoice(State(app): State<App>, p: Proj, Form(f): Form<InvoiceForm>) -> DashResult {
    p.require(ProjectRole::Developer)?;
    let back = format!("{}/invoices", p.base(&app));
    let Ok(amount) = f.amount.trim().parse::<Decimal>() else {
        return Ok(err_redirect(&app, &back, "Enter an amount."));
    };
    let inv = match payments::create_invoice(
        &app,
        p.project.project_id,
        CreateInvoice {
            price_amount: amount,
            price_currency: f.currency,
            order_description: Some(f.description).filter(|s| !s.trim().is_empty()),
            ..Default::default()
        },
    )
    .await
    {
        Ok(i) => i,
        Err(e) => return Ok(err_redirect(&app, &back, &e.public_message())),
    };
    p.audit(&app, "invoice.create", "invoice", Some(&inv.invoice_id.to_string()), serde_json::json!({})).await;
    let domain = app.store.checkout_domain(p.project.project_id).await?;
    invoices_view(&app, &p, crate::http::invoice_url(&app, domain.as_deref(), &inv.token)).await
}
