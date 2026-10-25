//! Project settings for the checkout: branding and logo, return URLs, the exchange and the
//! checkout domain. A live preview renders the checkout with sample data.

use askama::Template;
use axum::{
    Form, Router,
    extract::{DefaultBodyLimit, Multipart, Query, State},
    http::{HeaderValue, header},
    response::Response,
    routing::{get, post},
};
use chrono::{Duration, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;

use super::{DashResult, Proj, ProjectRole, Shell, err_redirect, ok_redirect, page, render};
use crate::{
    app::App,
    branding::{self, Look},
    db::{BrandingRow, BrandingUpdate, PaymentRow},
    domain::{PaymentStatus, Settlement},
    error::Error,
    exchange,
    http::checkout::{self, PaymentFacts, PaymentPage},
};

pub fn routes() -> Router<App> {
    Router::new()
        .route("/p/{pid}/settings/checkout", get(checkout_page).post(save_checkout))
        .route("/p/{pid}/settings/checkout/logo", post(save_logo).layer(DefaultBodyLimit::max(branding::MAX_LOGO_BYTES + 256 * 1024)))
        .route("/p/{pid}/settings/preview", get(preview))
        .route("/p/{pid}/settings/preview/status", get(preview_status))
        .route("/p/{pid}/settings/exchange", get(exchange_page).post(save_exchange))
        .route("/p/{pid}/settings/exchange/remove-key", post(remove_exchange_key))
        .route("/p/{pid}/settings/domain", get(domain_page).post(save_domain))
}

// ---- checkout branding -----------------------------------------------------------

#[derive(Template)]
#[template(path = "dashboard/settings_checkout.html")]
struct CheckoutPage {
    shell: Shell,
    pid: i64,
    tab: &'static str,
    b: BrandingRow,
    accent: String,
    hosts: String,
    logo: String,
    can_edit: bool,
    preview: String,
}

async fn checkout_page(State(app): State<App>, p: Proj) -> DashResult {
    let pid = p.project.project_id;
    let b = app.store.branding(pid).await?;
    let look = Look::new(&app, &b);
    let shell = Shell::new(&app, &p.user, Some(&p.project), "settings", "Checkout settings").await;
    Ok(page(
        &app,
        &CheckoutPage {
            shell,
            pid,
            tab: "checkout",
            accent: b.accent_color.clone().unwrap_or_default(),
            hosts: b.return_hosts.join("\n"),
            logo: look.logo,
            b,
            can_edit: p.role >= ProjectRole::Admin,
            preview: format!("{}/settings/preview?state=waiting", p.base(&app)),
        },
    ))
}

#[derive(Deserialize)]
struct CheckoutForm {
    #[serde(default)]
    accent: String,
    #[serde(default)]
    accent_default: Option<String>,
    #[serde(default)]
    theme: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    support_email: String,
    #[serde(default)]
    support_url: String,
    #[serde(default)]
    return_url: String,
    #[serde(default)]
    return_hosts: String,
}

fn opt(s: &str) -> Option<String> {
    Some(s.trim().to_string()).filter(|s| !s.is_empty())
}

fn check_link(app: &App, name: &str, v: &Option<String>) -> Result<(), Error> {
    crate::payments::check_url(app, name, v)
}

async fn save_checkout(State(app): State<App>, p: Proj, Form(f): Form<CheckoutForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/settings/checkout", p.base(&app));
    let accent = match (&f.accent_default, opt(&f.accent)) {
        (Some(_), _) | (None, None) => None,
        (None, Some(a)) => match branding::parse_hex_color(&a) {
            Some(c) => Some(c),
            None => return Ok(err_redirect(&app, &back, "Enter the accent colour as a hex value, for example #1d4ed8.")),
        },
    };
    let theme = if branding::THEMES.contains(&f.theme.as_str()) { f.theme.clone() } else { "auto".into() };
    let message = opt(&f.message);
    if message.as_ref().is_some_and(|m| m.chars().count() > branding::MAX_MESSAGE) {
        return Ok(err_redirect(&app, &back, "The checkout message has more than 280 characters."));
    }
    let support_email = opt(&f.support_email);
    if support_email
        .as_ref()
        .is_some_and(|e| e.len() > 254 || !e.contains('@') || e.contains(char::is_whitespace) || e.contains(['<', '>', '"']))
    {
        return Ok(err_redirect(&app, &back, "Enter a valid support email address."));
    }
    let (support_url, return_url) = (opt(&f.support_url), opt(&f.return_url));
    for (name, v) in [("Support URL", &support_url), ("Return URL", &return_url)] {
        if check_link(&app, name, v).is_err() {
            return Ok(err_redirect(&app, &back, &format!("{name}: enter an https:// URL.")));
        }
    }
    let return_hosts = match branding::parse_hosts(&f.return_hosts) {
        Ok(h) => h,
        Err(e) => return Ok(err_redirect(&app, &back, &e.public_message())),
    };
    if let Some(u) = &return_url
        && !branding::host_allowed(&return_hosts, u)
    {
        return Ok(err_redirect(&app, &back, "The return URL must use one of the allowed return hosts."));
    }
    let pid = p.project.project_id;
    let update = BrandingUpdate { accent_color: accent, theme, message, support_email, support_url, return_url, return_hosts };
    app.store.save_branding(pid, &update).await?;
    p.audit(
        &app,
        "project.checkout",
        "project",
        Some(&pid.to_string()),
        serde_json::json!({ "accent": update.accent_color, "theme": update.theme, "return_url": update.return_url, "return_hosts": update.return_hosts }),
    )
    .await;
    Ok(ok_redirect(&app, &back, "Checkout settings saved."))
}

/// Upload a logo, fetch it from an https URL, or remove it.
async fn save_logo(State(app): State<App>, p: Proj, mut form: Multipart) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/settings/checkout", p.base(&app));
    let mut file: Option<Vec<u8>> = None;
    let mut url = String::new();
    let mut remove = false;
    loop {
        let field = match form.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(_) => return Ok(err_redirect(&app, &back, "The upload failed. Use a file of at most 2 MB.")),
        };
        match field.name().unwrap_or("") {
            "logo" => match field.bytes().await {
                Ok(b) if !b.is_empty() => file = Some(b.to_vec()),
                Ok(_) => {}
                Err(_) => return Ok(err_redirect(&app, &back, "The upload failed. Use a file of at most 2 MB.")),
            },
            "logo_url" => url = field.text().await.unwrap_or_default().trim().to_string(),
            "remove" => remove = true,
            _ => {}
        }
    }
    let pid = p.project.project_id;
    if remove {
        app.store.set_logo(pid, None, None).await?;
        p.audit(&app, "project.logo_remove", "project", Some(&pid.to_string()), serde_json::json!({})).await;
        return Ok(ok_redirect(&app, &back, "Logo removed."));
    }
    let (bytes, source_url) = match (file, url.is_empty()) {
        (Some(b), _) => (b, None),
        (None, false) => {
            let u = Some(url.clone());
            if check_link(&app, "Logo URL", &u).is_err() || !url.starts_with("https://") {
                return Ok(err_redirect(&app, &back, "Logo URL: enter an https:// URL."));
            }
            match branding::fetch_logo(&app, &url).await {
                Ok(b) => (b, Some(url.clone())),
                Err(e) => return Ok(err_redirect(&app, &back, &e.public_message())),
            }
        }
        (None, true) => return Ok(err_redirect(&app, &back, "Choose an image file or enter a URL.")),
    };
    let (png, hash) = match branding::process_logo(&bytes) {
        Ok(r) => r,
        Err(e) => return Ok(err_redirect(&app, &back, &e.public_message())),
    };
    app.store.set_logo(pid, Some((&png, &hash)), source_url.as_deref()).await?;
    p.audit(&app, "project.logo", "project", Some(&pid.to_string()), serde_json::json!({ "hash": hash, "url": source_url })).await;
    Ok(ok_redirect(&app, &back, "Logo saved."))
}

// ---- preview ---------------------------------------------------------------------

const PREVIEW_CSP: &str = "default-src 'none'; style-src 'self'; style-src-attr 'unsafe-inline'; img-src 'self' data:; \
    font-src 'self'; frame-src 'self'; form-action 'none'; frame-ancestors 'self'; base-uri 'none'";

fn framed(mut r: Response) -> Response {
    let h = r.headers_mut();
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(PREVIEW_CSP));
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("SAMEORIGIN"));
    r
}

#[derive(Deserialize, Default)]
struct PreviewQuery {
    #[serde(default)]
    state: String,
    #[serde(default)]
    seen: String,
}

/// A sample payment in the given state.
fn sample(state: &str, an_id: i64, network_id: i64) -> PaymentRow {
    let now = Utc::now();
    let pay_amount = Decimal::new(153846, 8);
    let (status, confirmations, paid) = match state {
        "confirming" => (PaymentStatus::Confirming, 1, pay_amount),
        "partial" => (PaymentStatus::PartiallyPaid, 1, Decimal::new(61538, 8)),
        "paid" => (PaymentStatus::Finished, 3, pay_amount),
        "expired" => (PaymentStatus::Expired, 0, Decimal::ZERO),
        _ => (PaymentStatus::Waiting, 0, Decimal::ZERO),
    };
    PaymentRow {
        payment_id: 0,
        project_id: 0,
        invoice_id: None,
        token: "preview".into(),
        asset_network_id: an_id,
        network_id,
        purchase_id: "0".into(),
        order_id: Some("1042".into()),
        order_description: None,
        price_amount: Decimal::new(100, 0),
        price_currency: "usd".into(),
        pay_address: "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh".into(),
        payin_extra_id: None,
        pay_amount,
        actually_paid: paid,
        outcome_amount: None,
        network_fee_amount: Decimal::ZERO,
        status,
        status_seq: 0,
        settlement: Settlement::None,
        confirmations,
        ipn_callback_url: None,
        success_url: Some("https://shop.example/thanks".into()),
        cancel_url: None,
        rate: None,
        expires_at: if status == PaymentStatus::Expired { now - Duration::minutes(1) } else { now + Duration::minutes(42) },
        paid_at: None,
        created_at: now - Duration::minutes(18),
        updated_at: now,
    }
}

async fn preview(State(app): State<App>, p: Proj, Query(q): Query<PreviewQuery>) -> DashResult {
    let pid = p.project.project_id;
    let look = Look::load(&app, pid).await?;
    let an = app.store.asset_network_by_ticker("btc").await?;
    let pay = sample(&q.state, an.asset_network_id, an.network.network_id);
    let mut tpl = PaymentPage::new(&app, look, &pay, &an);
    tpl.frame_src = format!("{}/settings/preview/status?state={}", p.base(&app), q.state);
    tpl.wallet_link = "#".into();
    tpl.exchange_href = if q.state == "waiting" || q.state.is_empty() { "#".into() } else { String::new() };
    Ok(framed(render(&tpl)))
}

async fn preview_status(State(app): State<App>, p: Proj, Query(q): Query<PreviewQuery>) -> DashResult {
    let pid = p.project.project_id;
    let look = Look::load(&app, pid).await?;
    let mut an = app.store.asset_network_by_ticker("btc").await?;
    an.network.confirmations_required = 3;
    let pay = sample(&q.state, an.asset_network_id, an.network.network_id);
    let facts = PaymentFacts { status_changed_at: Utc::now(), return_url: None, invoice_href: String::new() };
    let mut v = checkout::payment_frame(&app, look, &pay, &an, &facts, &q.seen);
    if !v.refresh_url.is_empty() {
        let key = v.refresh_url.split("seen=").nth(1).unwrap_or("").to_string();
        v.refresh_url = format!("{}/settings/preview/status?state={}&seen={key}", p.base(&app), q.state);
    }
    for a in &mut v.actions {
        a.href = "#".into();
    }
    Ok(framed(render(&v)))
}

// ---- exchange --------------------------------------------------------------------

#[derive(Template)]
#[template(path = "dashboard/settings_exchange.html")]
struct ExchangePage {
    shell: Shell,
    pid: i64,
    tab: &'static str,
    enabled: bool,
    key_set: bool,
    markup: String,
    min_kyc: String,
    coins: String,
    coin_count: usize,
    supported: bool,
    can_edit: bool,
    markups: Vec<&'static str>,
    ratings: Vec<&'static str>,
}

async fn exchange_page(State(app): State<App>, p: Proj) -> DashResult {
    let pid = p.project.project_id;
    let s = app.store.exchange_settings(pid).await?;
    let coin_count = app.store.exchange_coins().await?.len();
    let shell = Shell::new(&app, &p.user, Some(&p.project), "settings", "Exchange settings").await;
    Ok(page(
        &app,
        &ExchangePage {
            shell,
            pid,
            tab: "exchange",
            enabled: s.enabled,
            key_set: s.key_sealed.is_some(),
            markup: s.markup.normalize().to_string(),
            min_kyc: s.min_kyc,
            coins: s.coins.join(", "),
            coin_count,
            supported: exchange::supported(&app),
            can_edit: p.role >= ProjectRole::Admin,
            markups: exchange::MARKUPS.to_vec(),
            ratings: exchange::KYC_RATINGS.to_vec(),
        },
    ))
}

#[derive(Deserialize)]
struct ExchangeForm {
    #[serde(default)]
    enabled: Option<String>,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    markup: String,
    #[serde(default)]
    min_kyc: String,
    #[serde(default)]
    coins: String,
    #[serde(default)]
    totp: String,
}

async fn save_exchange(State(app): State<App>, p: Proj, Form(f): Form<ExchangeForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/settings/exchange", p.base(&app));
    if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    let Some(markup) = exchange::MARKUPS.iter().find(|m| **m == f.markup.trim()).and_then(|m| m.parse::<Decimal>().ok()) else {
        return Ok(err_redirect(&app, &back, "Choose a markup of 0, 1, 1.65 or 3 %."));
    };
    let min_kyc = f.min_kyc.trim().to_ascii_uppercase();
    if !exchange::KYC_RATINGS.contains(&min_kyc.as_str()) {
        return Ok(err_redirect(&app, &back, "Choose a KYC rating from A to D."));
    }
    let mut coins: Vec<String> = Vec::new();
    for c in f.coins.split([',', ' ', '\n']).map(|c| c.trim().to_ascii_lowercase()).filter(|c| !c.is_empty()) {
        if c.len() > 40 || !c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b':' || b == b'-' || b == b'_') {
            return Ok(err_redirect(&app, &back, &format!("`{c}` is not a coin ticker.")));
        }
        if !coins.contains(&c) {
            coins.push(c);
        }
    }
    let pid = p.project.project_id;
    let key = f.api_key.trim();
    let sealed = if key.is_empty() {
        None
    } else {
        if key.len() > 200 || key.contains(char::is_whitespace) {
            return Ok(err_redirect(&app, &back, "The API key is not valid."));
        }
        if exchange::supported(&app) {
            let client = exchange::client_for_key(&app, key);
            if let Err(e) = exchange::refresh_coins_with(&app, &client).await {
                return Ok(err_redirect(&app, &back, &format!("The exchange did not accept the key: {}", e.public_message())));
            }
        }
        Some(app.sealer.seal_str("trocador_key", key))
    };
    let enabled = f.enabled.is_some();
    let has_key = sealed.is_some() || app.store.exchange_settings(pid).await?.key_sealed.is_some();
    if enabled && !has_key {
        return Ok(err_redirect(&app, &back, "Enter a Trocador API key to turn the exchange on."));
    }
    app.store.save_exchange_settings(pid, enabled, sealed.as_deref(), markup, &min_kyc, &coins).await?;
    if enabled && let Err(e) = exchange::refresh_coins_if_stale(&app).await {
        tracing::warn!(error = %e, "exchange coin list refresh failed");
    }
    p.audit(
        &app,
        "project.exchange",
        "project",
        Some(&pid.to_string()),
        serde_json::json!({ "enabled": enabled, "key_changed": sealed.is_some(), "markup": markup.to_string(), "min_kyc": min_kyc, "coins": coins }),
    )
    .await;
    Ok(ok_redirect(&app, &back, "Exchange settings saved."))
}

#[derive(Deserialize)]
struct TotpForm {
    #[serde(default)]
    totp: String,
}

async fn remove_exchange_key(State(app): State<App>, p: Proj, Form(f): Form<TotpForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/settings/exchange", p.base(&app));
    if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    let pid = p.project.project_id;
    app.store.clear_exchange_key(pid).await?;
    p.audit(&app, "project.exchange_key_remove", "project", Some(&pid.to_string()), serde_json::json!({})).await;
    Ok(ok_redirect(&app, &back, "API key removed. The exchange is off."))
}

// ---- checkout domain ---------------------------------------------------------------

#[derive(Template)]
#[template(path = "dashboard/settings_domain.html")]
struct DomainPage {
    shell: Shell,
    pid: i64,
    tab: &'static str,
    domain: String,
    site_host: String,
    is_admin: bool,
    example: String,
}

async fn domain_page(State(app): State<App>, p: Proj) -> DashResult {
    let pid = p.project.project_id;
    let domain = app.store.checkout_domain(pid).await?.unwrap_or_default();
    let shell = Shell::new(&app, &p.user, Some(&p.project), "settings", "Checkout domain").await;
    Ok(page(
        &app,
        &DomainPage {
            shell,
            pid,
            tab: "domain",
            example: crate::http::payment_url(&app, Some(if domain.is_empty() { "pay.example.com" } else { &domain }), "…"),
            domain,
            site_host: app.config.site_host(),
            is_admin: p.user.is_admin(),
        },
    ))
}

#[derive(Deserialize)]
struct DomainForm {
    #[serde(default)]
    domain: String,
    #[serde(default)]
    totp: String,
}

async fn save_domain(State(app): State<App>, p: Proj, Form(f): Form<DomainForm>) -> DashResult {
    p.user.require_admin()?;
    let back = format!("{}/settings/domain", p.base(&app));
    if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    let raw = f.domain.trim().trim_end_matches('.').to_ascii_lowercase();
    let raw = raw.strip_prefix("https://").unwrap_or(&raw).trim_end_matches('/').to_string();
    let domain = (!raw.is_empty()).then_some(raw);
    if let Some(d) = &domain {
        if !branding::valid_host(d) {
            return Ok(err_redirect(&app, &back, "Enter a host name, for example pay.example.com."));
        }
        if *d == app.config.site_host() {
            return Ok(err_redirect(&app, &back, "The checkout domain must differ from the site host."));
        }
    }
    let pid = p.project.project_id;
    if let Err(e) = app.store.set_checkout_domain(pid, domain.as_deref()).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    p.audit(&app, "project.checkout_domain", "project", Some(&pid.to_string()), serde_json::json!({ "domain": domain })).await;
    Ok(ok_redirect(&app, &back, if domain.is_some() { "Checkout domain saved." } else { "Checkout domain removed." }))
}
