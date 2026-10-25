//! Customer pages. They work with JavaScript off: live status comes from an iframe that
//! reloads itself with `<meta http-equiv="refresh">`, and all motion is CSS.
//!
//! The frame reloads every 4 seconds, so its animations restart. Looping animations take
//! their phase from the server clock (`--phase`), so a reload does not show. Animations of
//! a state change play only on the first frame that shows the new state: each frame links
//! to the next with the state it showed (`?seen=`).

use askama::Template;
use axum::{
    Form, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use rust_decimal::{Decimal, prelude::ToPrimitive};
use serde::Deserialize;

use crate::{
    app::App,
    branding::Look,
    db::{ExchangeCoin, InvoiceRow, PaymentRow, TradeRow},
    domain::{AssetNetwork, PaymentStatus},
    error::Error,
    exchange,
    http::{dashboard::render, glyphs},
    payments,
};

pub fn routes() -> Router<App> {
    Router::new()
        .route("/i/{token}", get(invoice_page).post(choose_currency))
        .route("/pay/{token}", get(payment_page))
        .route("/pay/{token}/status", get(status_frame))
        .route("/x/{token}", get(exchange_page).post(start_swap))
        .route("/x/{token}/status", get(trade_frame))
        .route("/x/hook/{secret}", post(trade_hook))
        .route("/branding/{project_id}/{file}", get(logo))
}

/// Seconds between two status frame loads.
pub const REFRESH_SECS: u32 = 4;
/// Every looping animation period divides this, so one phase value serves them all.
const PHASE_PERIOD_MS: i64 = 240_000;

// ---- formatting ------------------------------------------------------------------

/// Inline SVG QR code. No script, no external image.
pub fn qr_svg(data: &str) -> String {
    use qrcode::{EcLevel, QrCode, render::svg};
    QrCode::with_error_correction_level(data.as_bytes(), EcLevel::M)
        .map(|code| {
            code.render::<svg::Color>()
                .min_dimensions(232, 232)
                .quiet_zone(false)
                .dark_color(svg::Color("#0a0a0a"))
                .light_color(svg::Color("#ffffff"))
                .build()
        })
        .unwrap_or_default()
}

pub fn fmt_amount(d: Decimal) -> String {
    d.normalize().to_string()
}

pub fn fmt_fiat(d: Decimal, currency: &str) -> String {
    let c = currency.to_ascii_uppercase();
    let d = d.round_dp(2);
    let (int, frac) = {
        let s = format!("{d:.2}");
        match s.split_once('.') {
            Some((i, f)) => (i.to_string(), f.to_string()),
            None => (s, "00".into()),
        }
    };
    let neg = int.starts_with('-');
    let digits = int.trim_start_matches('-');
    let mut grouped = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{}{grouped}.{frac} {c}", if neg { "-" } else { "" })
}

/// A rate with `digits` significant digits, without trailing zeros.
fn sig(d: Decimal, digits: i32) -> String {
    let f = d.to_f64().unwrap_or(0.0);
    if f <= 0.0 {
        return "0".into();
    }
    let decimals = (digits - 1 - f.log10().floor() as i32).clamp(0, 12) as usize;
    let s = format!("{f:.decimals$}");
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
}

/// Integer amount in the smallest unit (wei, nanoton, ...).
fn atomic(amount: Decimal, decimals: i32) -> String {
    let scaled = amount * Decimal::from(10u64.pow(decimals.clamp(0, 18) as u32));
    let scaled = if decimals > 18 { scaled * Decimal::from(10u64.pow((decimals - 18) as u32)) } else { scaled };
    scaled.trunc().normalize().to_string()
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// What the customer must still send: the rest of a partly paid payment.
fn to_send(p: &PaymentRow) -> Decimal {
    if p.status == PaymentStatus::PartiallyPaid { (p.pay_amount - p.actually_paid).max(Decimal::ZERO) } else { p.pay_amount }
}

/// "Open in wallet" link: BIP21, EIP-681, Solana Pay and the like.
pub fn wallet_link(p: &PaymentRow, an: &AssetNetwork) -> String {
    let send = to_send(p);
    let amount = fmt_amount(send);
    let addr = &p.pay_address;
    let memo = p.payin_extra_id.as_deref().filter(|m| !m.is_empty());
    let contract = an.contract_address.as_deref();
    match an.network.family.as_str() {
        "bitcoin" => format!("bitcoin:{addr}?amount={amount}"),
        "monero" => format!("monero:{addr}?tx_amount={amount}"),
        "evm" => {
            let chain = an.network.chain_id.unwrap_or(1);
            let units = atomic(send, an.decimals);
            match contract {
                Some(c) => format!("ethereum:{c}@{chain}/transfer?address={addr}&uint256={units}"),
                None => format!("ethereum:{addr}@{chain}?value={units}"),
            }
        }
        "solana" => match contract {
            Some(mint) => format!("solana:{addr}?amount={amount}&spl-token={mint}"),
            None => format!("solana:{addr}?amount={amount}"),
        },
        "ton" => {
            let mut s = format!("ton://transfer/{addr}?amount={}", atomic(send, an.decimals));
            if let Some(m) = memo {
                s.push_str(&format!("&text={}", enc(m)));
            }
            s
        }
        "tron" => match contract {
            Some(c) => format!("tron:{addr}?amount={amount}&token={c}"),
            None => format!("tron:{addr}?amount={amount}"),
        },
        _ => addr.clone(),
    }
}

/// What the QR code holds. Plain addresses where wallets disagree on URIs.
fn qr_payload(p: &PaymentRow, an: &AssetNetwork) -> String {
    match an.network.family.as_str() {
        "bitcoin" | "monero" | "solana" | "ton" => wallet_link(p, an),
        _ => p.pay_address.clone(),
    }
}

fn network_name(an: &AssetNetwork) -> String {
    match an.network.name.as_str() {
        "TRON" => "Tron".into(),
        other => other.replace("TRON ", "Tron "),
    }
}

/// Token standard shown next to the network, when customers know it by that name.
fn token_standard(code: &str) -> Option<&'static str> {
    match code {
        "ETH" => Some("ERC-20"),
        "BSC" => Some("BEP-20"),
        "TRON" => Some("TRC-20"),
        "SOL" => Some("SPL"),
        _ => None,
    }
}

/// "Tron (TRC-20)" for tokens, "Bitcoin" for native coins.
fn network_label(an: &AssetNetwork) -> String {
    match (an.is_native(), token_standard(&an.network.code)) {
        (false, Some(std)) => format!("{} ({std})", network_name(an)),
        _ => network_name(an),
    }
}

fn network_warning(an: &AssetNetwork) -> String {
    format!("Send {} on {} only. Funds sent on another network are lost.", an.asset.symbol, network_label(an))
}

/// Negative delay that puts every looping animation where the server clock says it is.
fn phase_style() -> String {
    let ms = Utc::now().timestamp_millis().rem_euclid(PHASE_PERIOD_MS);
    format!("--phase:-{}.{:03}s", ms / 1000, ms % 1000)
}

// ---- errors ----------------------------------------------------------------------

#[derive(Template)]
#[template(path = "checkout/error.html")]
struct ErrorPage {
    look: Look,
    base: String,
    title: String,
    message: String,
}

/// An error on a customer page: an HTML page, not JSON.
pub struct PageError(Error);

impl From<Error> for PageError {
    fn from(e: Error) -> Self {
        PageError(e)
    }
}

impl IntoResponse for PageError {
    fn into_response(self) -> Response {
        self.0.log();
        let status = self.0.status();
        let message = match self.0 {
            Error::NotFound => "This payment link does not exist. Check the link, or ask the shop for a new one.".to_string(),
            ref e => e.public_message(),
        };
        let page = ErrorPage {
            look: Look { store: String::new(), theme: "auto".into(), ..Default::default() },
            base: crate::http::BASE_PATH.get().cloned().unwrap_or_default(),
            title: if status == StatusCode::NOT_FOUND { "Not found".into() } else { "Something went wrong".into() },
            message,
        };
        let mut r = render(&page);
        *r.status_mut() = status;
        r
    }
}

type PageResult = Result<Response, PageError>;

/// On a checkout domain, show only the pages of its own project.
async fn check_host(app: &App, headers: &HeaderMap, project_id: i64) -> Result<(), Error> {
    match crate::http::checkout_domain_project(app, headers).await? {
        Some(pid) if pid != project_id => Err(Error::NotFound),
        _ => Ok(()),
    }
}

// ---- invoice -----------------------------------------------------------------------

struct CoinOption {
    ticker: String,
    symbol: String,
    name: String,
    glyph: String,
}

struct NetworkGroup {
    name: String,
    glyph: String,
    coins: Vec<CoinOption>,
}

#[derive(Template)]
#[template(path = "checkout/invoice.html")]
struct InvoicePage {
    look: Look,
    base: String,
    title: String,
    token: String,
    amount: String,
    description: String,
    groups: Vec<NetworkGroup>,
    error: String,
    expired: bool,
    cancel_url: String,
    exchange: bool,
}

/// The payment coin the exchange option of an invoice settles in, if any.
async fn exchange_settle_ticker(app: &App, project_id: i64, enabled: &[AssetNetwork]) -> Result<Option<String>, Error> {
    let settings = app.store.exchange_settings(project_id).await?;
    if exchange::client(app, &settings)?.is_none() {
        return Ok(None);
    }
    for t in exchange::SETTLE_PREFERENCE {
        if let Some(an) = enabled.iter().find(|a| a.ticker == t)
            && exchange::target_coin(app, an).await?.is_some()
        {
            return Ok(Some(an.ticker.clone()));
        }
    }
    Ok(None)
}

async fn enabled_coins(app: &App, project_id: i64) -> Result<Vec<AssetNetwork>, Error> {
    let enabled = crate::payments::offered_asset_network_ids(app, Some(project_id)).await?;
    Ok(app.store.list_asset_networks().await?.into_iter().filter(|an| enabled.contains(&an.asset_network_id)).collect())
}

async fn invoice_view(app: &App, inv: &InvoiceRow, error: &str) -> PageResult {
    let look = Look::load(app, inv.project_id).await?;
    let coins = enabled_coins(app, inv.project_id).await?;
    let mut groups: Vec<NetworkGroup> = Vec::new();
    for an in coins.iter().filter(|an| inv.pay_currency.as_ref().is_none_or(|t| *t == an.ticker)) {
        let opt = CoinOption {
            ticker: an.ticker.clone(),
            symbol: an.asset.symbol.clone(),
            name: an.asset.name.clone(),
            glyph: glyphs::coin(&an.asset.symbol).unwrap_or_default(),
        };
        match groups.iter_mut().find(|g| g.name == network_name(an)) {
            Some(g) => g.coins.push(opt),
            None => {
                let native = coins.iter().find(|c| c.network.network_id == an.network.network_id && c.is_native());
                let native_symbol = native.map(|n| n.asset.symbol.as_str()).unwrap_or(an.asset.symbol.as_str());
                groups.push(NetworkGroup {
                    name: network_name(an),
                    glyph: glyphs::network(&an.network.code, native_symbol).unwrap_or_default(),
                    coins: vec![opt],
                })
            }
        }
    }
    let exchange = exchange_settle_ticker(app, inv.project_id, &coins).await?.is_some();
    let page = InvoicePage {
        title: format!("Pay {}", look.store),
        look,
        base: app.config.base_path().to_string(),
        token: inv.token.clone(),
        amount: fmt_fiat(inv.price_amount, &inv.price_currency),
        description: inv.order_description.clone().unwrap_or_default(),
        groups,
        error: error.to_string(),
        expired: inv.expires_at < Utc::now(),
        cancel_url: inv.cancel_url.clone().unwrap_or_default(),
        exchange,
    };
    Ok(render(&page))
}

async fn invoice_page(State(app): State<App>, headers: HeaderMap, Path(token): Path<String>) -> PageResult {
    let inv = app.store.invoice_by_token(&token).await?.ok_or(Error::NotFound)?;
    check_host(&app, &headers, inv.project_id).await?;
    // An invoice for one coin goes straight to its payment.
    if let Some(ticker) = inv.pay_currency.clone()
        && inv.expires_at > Utc::now()
    {
        return match payments::pay_invoice(&app, &inv, &ticker, None).await {
            Ok(p) => Ok(Redirect::to(&app.path(&format!("/pay/{}", p.token))).into_response()),
            Err(e) => invoice_view(&app, &inv, &e.public_message()).await,
        };
    }
    invoice_view(&app, &inv, "").await
}

#[derive(Deserialize)]
struct ChooseForm {
    currency: String,
}

/// `currency=any` pays through the exchange.
async fn choose_currency(State(app): State<App>, headers: HeaderMap, Path(token): Path<String>, Form(f): Form<ChooseForm>) -> PageResult {
    let inv = app.store.invoice_by_token(&token).await?.ok_or(Error::NotFound)?;
    check_host(&app, &headers, inv.project_id).await?;
    let (ticker, swap) = if f.currency == "any" {
        let coins = enabled_coins(&app, inv.project_id).await?;
        match exchange_settle_ticker(&app, inv.project_id, &coins).await? {
            Some(t) => (t, true),
            None => return invoice_view(&app, &inv, "Paying with another coin is not available.").await,
        }
    } else {
        (f.currency, false)
    };
    match payments::pay_invoice(&app, &inv, &ticker, None).await {
        Ok(p) if swap => Ok(Redirect::to(&app.path(&format!("/x/{}", p.token))).into_response()),
        Ok(p) => Ok(Redirect::to(&app.path(&format!("/pay/{}", p.token))).into_response()),
        Err(e) => invoice_view(&app, &inv, &e.public_message()).await,
    }
}

// ---- payment -------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "checkout/payment.html")]
pub struct PaymentPage {
    pub look: Look,
    pub base: String,
    pub title: String,
    pub amount: String,
    pub symbol: String,
    pub glyph: String,
    pub network: String,
    pub network_glyph: String,
    pub warning: String,
    pub fiat: String,
    pub order: String,
    pub address: String,
    pub memo: String,
    pub qr: String,
    pub wallet_link: String,
    pub frame_src: String,
    pub invoice_href: String,
    pub exchange_href: String,
    pub cancel_href: String,
    /// Seconds until the payment window closes; the page dims the address then.
    pub seconds_left: i64,
    /// The payment was final when the page loaded: show the result only.
    pub done: bool,
    pub eyebrow: &'static str,
}

impl PaymentPage {
    pub fn new(app: &App, look: Look, p: &PaymentRow, an: &AssetNetwork) -> Self {
        PaymentPage {
            title: format!("Pay {} {} to {}", fmt_amount(p.pay_amount), an.asset.symbol, look.store),
            look,
            base: app.config.base_path().to_string(),
            amount: fmt_amount(to_send(p)),
            symbol: an.asset.symbol.clone(),
            glyph: glyphs::coin(&an.asset.symbol).unwrap_or_default(),
            network: network_label(an),
            network_glyph: glyphs::network(&an.network.code, &an.asset.symbol).unwrap_or_default(),
            warning: network_warning(an),
            fiat: fmt_fiat(p.price_amount, &p.price_currency),
            order: p.order_description.clone().or_else(|| p.order_id.clone().map(|o| format!("Order {o}"))).unwrap_or_default(),
            address: p.pay_address.clone(),
            memo: p.payin_extra_id.clone().unwrap_or_default(),
            qr: qr_svg(&qr_payload(p, an)),
            wallet_link: wallet_link(p, an),
            frame_src: app.path(&format!("/pay/{}/status", p.token)),
            invoice_href: String::new(),
            exchange_href: String::new(),
            cancel_href: p.cancel_url.clone().unwrap_or_default(),
            seconds_left: (p.expires_at - Utc::now()).num_seconds().max(0),
            done: p.status.is_terminal(),
            eyebrow: match p.status {
                PaymentStatus::Finished => "Amount paid",
                PaymentStatus::Refunded => "Amount refunded",
                PaymentStatus::PartiallyPaid => "Left to pay",
                _ => "Amount due",
            },
        }
    }
}

async fn payment_page(State(app): State<App>, headers: HeaderMap, Path(token): Path<String>) -> PageResult {
    let p = app.store.payment_by_token(&token).await?.ok_or(Error::NotFound)?;
    check_host(&app, &headers, p.project_id).await?;
    let an = app.store.asset_network(p.asset_network_id).await?;
    let look = Look::load(&app, p.project_id).await?;
    let mut page = PaymentPage::new(&app, look, &p, &an);
    if let Some(id) = p.invoice_id
        && let Some(inv) = app.store.invoice(p.project_id, id).await?
        && inv.pay_currency.is_none()
        && inv.expires_at > Utc::now()
        && !p.status.is_terminal()
    {
        page.invoice_href = app.path(&format!("/i/{}", inv.token));
    }
    if exchange_offer(&app, &p, &an).await? {
        page.exchange_href = app.path(&format!("/x/{}", p.token));
    }
    Ok(render(&page))
}

/// True when the customer can pay this payment with another coin.
async fn exchange_offer(app: &App, p: &PaymentRow, an: &AssetNetwork) -> Result<bool, Error> {
    if !exchange::payment_open(p) {
        return Ok(false);
    }
    let settings = app.store.exchange_settings(p.project_id).await?;
    Ok(exchange::client(app, &settings)?.is_some() && exchange::target_coin(app, an).await?.is_some())
}

// ---- status frame --------------------------------------------------------------------

/// One cell of the 7x7 grid in the status frame.
pub struct Cell {
    pub class: &'static str,
    pub style: String,
}

const GRID: i32 = 7;
/// The brand mark on the grid: three cells on top, two below the middle one.
const T_CELLS: [(i32, i32); 5] = [(2, 2), (3, 2), (4, 2), (3, 3), (3, 4)];

/// Grid cells. `filled` cells (not counting the mark) fill outward from the mark;
/// those after `old_filled` are new and animate in.
pub fn grid(filled: usize, old_filled: usize) -> Vec<Cell> {
    let mut order: Vec<(i32, i32)> = (0..GRID * GRID).map(|i| (i % GRID, i / GRID)).filter(|c| !T_CELLS.contains(c)).collect();
    // Ring by ring from the centre, clockwise from the top.
    order.sort_by(|a, b| {
        let key = |&(x, y): &(i32, i32)| {
            let (dx, dy) = (f64::from(x - 3), f64::from(y - 3));
            ((dx.abs().max(dy.abs())) as i32, (dx.atan2(-dy) + std::f64::consts::TAU) % std::f64::consts::TAU)
        };
        let (ka, kb) = (key(a), key(b));
        ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1))
    });
    (0..GRID * GRID)
        .map(|i| {
            let (x, y) = (i % GRID, i / GRID);
            let d = (x - 3).abs().max((y - 3).abs());
            let rank = order.iter().position(|c| *c == (x, y));
            let class = match rank {
                None => "t",
                Some(r) if r < old_filled.min(filled) => "f",
                Some(r) if r < filled => "f n",
                Some(_) => "",
            };
            let delay = rank.map(|r| r.saturating_sub(old_filled)).unwrap_or(0);
            Cell { class, style: format!("--d:{d};--w:{};--i:{delay}", x + y) }
        })
        .collect()
}

pub const GRID_FREE: usize = (GRID * GRID) as usize - T_CELLS.len();

fn cells_for(fraction: f64) -> usize {
    ((fraction.clamp(0.0, 1.0) * GRID_FREE as f64).round() as usize).min(GRID_FREE)
}

/// A link or button in the status frame. It opens in the whole window.
pub struct Action {
    pub label: String,
    pub href: String,
    pub primary: bool,
}

/// An estimate of the time left for confirmations: a long animation that a reload must not
/// restart, so it starts in the past (`-elapsed`).
pub struct Eta {
    pub seconds: i64,
    pub elapsed: String,
}

impl Eta {
    fn new(seconds: i64, since: DateTime<Utc>) -> Self {
        let elapsed = (Utc::now() - since).num_milliseconds().max(0) as f64 / 1000.0;
        Eta { seconds: seconds.max(1), elapsed: format!("{elapsed:.1}") }
    }
}

/// Typical seconds between blocks.
fn block_secs(code: &str) -> i64 {
    match code {
        "BTC" => 600,
        "XMR" => 120,
        "ETH" => 12,
        "BSC" | "TRON" => 3,
        "BASE" | "POLYGON" => 2,
        "TON" => 5,
        _ => 1,
    }
}

/// Countdown to the end of the payment window.
pub struct Countdown {
    pub seconds: i64,
    /// Fraction of the window left, `0` to `1`.
    pub left: String,
    pub hours: bool,
}

impl Countdown {
    fn new(now: DateTime<Utc>, start: DateTime<Utc>, end: DateTime<Utc>) -> Option<Self> {
        let total = (end - start).num_seconds().max(1);
        let seconds = (end - now).num_seconds().clamp(0, total);
        (seconds > 0).then(|| Countdown { seconds, left: format!("{:.4}", seconds as f64 / total as f64), hours: seconds >= 3600 })
    }
}

#[derive(Template)]
#[template(path = "checkout/status.html")]
pub struct FrameView {
    pub look: Look,
    pub base: String,
    /// waiting, confirming, partial, confirmed, paid, expired, failed, refunded
    pub kind: &'static str,
    /// The frame shows this state for the first time: play its entrance.
    pub fresh: bool,
    pub title: String,
    pub detail: String,
    pub refresh_url: String,
    pub phase: String,
    pub cells: Vec<Cell>,
    /// Segment classes ("", "on", "on n"); empty for no segments.
    pub segs: Vec<&'static str>,
    pub progress: String,
    pub countdown: Option<Countdown>,
    pub eta: Option<Eta>,
    pub actions: Vec<Action>,
}

#[derive(Deserialize, Default)]
pub struct SeenQuery {
    #[serde(default)]
    seen: String,
}

/// State key a frame passes to the next one.
fn state_key(parts: &[&str]) -> String {
    parts.join("-")
}

/// Count in the previous state key's last part when it had the same prefix.
fn seen_count(seen: &str, prefix: &str) -> Option<usize> {
    seen.strip_prefix(prefix)?.strip_prefix('-')?.rsplit('-').next()?.parse().ok()
}

fn segments(n: usize, of: usize, old: usize) -> Vec<&'static str> {
    (0..of)
        .map(|i| {
            if i < old.min(n) {
                "on"
            } else if i < n {
                "on n"
            } else {
                ""
            }
        })
        .collect()
}

/// Where "Return to the store" goes: the payment's URL, else the project's at creation time.
fn return_target(primary: Option<&String>, stamped: &Option<String>) -> String {
    primary.or(stamped.as_ref()).cloned().unwrap_or_default()
}

pub struct PaymentFacts {
    pub status_changed_at: DateTime<Utc>,
    pub return_url: Option<String>,
    pub invoice_href: String,
}

/// Status frame contents for a payment.
pub fn payment_frame(app: &App, look: Look, p: &PaymentRow, an: &AssetNetwork, facts: &PaymentFacts, seen: &str) -> FrameView {
    let required = an.network.confirmations_required.max(1);
    let confs = p.confirmations.clamp(0, required);
    let sym = &an.asset.symbol;
    let store = look.store.clone();
    let now = Utc::now();
    let (kind, key) = match p.status {
        PaymentStatus::Waiting => ("waiting", state_key(&["waiting"])),
        PaymentStatus::Confirming => ("confirming", state_key(&["confirming", &confs.to_string()])),
        PaymentStatus::PartiallyPaid => ("partial", state_key(&["partial", &cells_for(ratio(p.actually_paid, p.pay_amount)).to_string()])),
        PaymentStatus::Confirmed | PaymentStatus::Sending => ("confirmed", state_key(&["confirmed"])),
        PaymentStatus::Finished => ("paid", state_key(&["paid"])),
        PaymentStatus::Expired => ("expired", state_key(&["expired"])),
        PaymentStatus::Failed => ("failed", state_key(&["failed"])),
        PaymentStatus::Refunded => ("refunded", state_key(&["refunded"])),
    };
    let fresh = seen != key;
    let mut v = FrameView {
        look,
        base: app.config.base_path().to_string(),
        kind,
        fresh,
        title: String::new(),
        detail: String::new(),
        refresh_url: if p.status.is_terminal() { String::new() } else { app.path(&format!("/pay/{}/status?seen={}", p.token, enc(&key))) },
        phase: phase_style(),
        cells: grid(0, 0),
        segs: vec![],
        progress: String::new(),
        countdown: None,
        eta: None,
        actions: vec![],
    };
    let ret = |label: String, url: String| (!url.is_empty()).then_some(Action { label, href: url, primary: true });
    match p.status {
        PaymentStatus::Waiting => {
            v.title = "Waiting for payment".into();
            v.detail = format!("Send {} {sym} to the address below.", fmt_amount(p.pay_amount));
            v.countdown = Countdown::new(now, p.created_at, p.expires_at);
        }
        PaymentStatus::Confirming => {
            v.title = "Payment received".into();
            v.detail =
                if required > 1 { "Waiting for network confirmations.".into() } else { "Waiting for a network confirmation.".into() };
            let old = if fresh { seen_count(seen, "confirming").unwrap_or(0) } else { confs as usize };
            v.cells = grid(cells_for(f64::from(confs) / f64::from(required)), cells_for(old as f64 / f64::from(required)));
            if required <= 12 {
                v.segs = segments(confs as usize, required as usize, old);
            }
            v.progress = format!("{confs} of {required}");
            v.eta = Some(Eta::new(i64::from(required) * block_secs(&an.network.code), facts.status_changed_at));
        }
        PaymentStatus::PartiallyPaid => {
            let rest = (p.pay_amount - p.actually_paid).max(Decimal::ZERO);
            v.title = "Partly paid".into();
            v.detail = format!("Received {} {sym}. Send {} {sym} more to the same address.", fmt_amount(p.actually_paid), fmt_amount(rest));
            let filled = cells_for(ratio(p.actually_paid, p.pay_amount));
            let old = if fresh { seen_count(seen, "partial").unwrap_or(0) } else { filled };
            v.cells = grid(filled, old);
            v.countdown = Countdown::new(now, p.created_at, p.expires_at);
        }
        PaymentStatus::Confirmed | PaymentStatus::Sending => {
            v.title = "Payment confirmed".into();
            v.detail = "Almost done.".into();
            v.cells = grid(GRID_FREE, if fresh { 0 } else { GRID_FREE });
        }
        PaymentStatus::Finished => {
            v.title = "Paid".into();
            v.detail = format!("{store} received your payment. Thank you.");
            v.cells = grid(GRID_FREE, GRID_FREE);
            v.actions.extend(ret(format!("Return to {store}"), return_target(p.success_url.as_ref(), &facts.return_url)));
        }
        PaymentStatus::Expired => {
            v.title = "Expired".into();
            v.detail = "The payment window closed. Do not send funds to this address.".into();
            if !facts.invoice_href.is_empty() {
                v.actions.push(Action { label: "Start again".into(), href: facts.invoice_href.clone(), primary: true });
            }
            v.actions.extend(ret(format!("Return to {store}"), return_target(p.cancel_url.as_ref(), &facts.return_url)).map(|mut a| {
                a.primary = facts.invoice_href.is_empty();
                a
            }));
        }
        PaymentStatus::Failed => {
            v.title = "Payment failed".into();
            v.detail = format!("This payment cannot be completed. Contact {store}.");
            v.actions.extend(ret(format!("Return to {store}"), return_target(p.cancel_url.as_ref(), &facts.return_url)));
        }
        PaymentStatus::Refunded => {
            v.title = "Refunded".into();
            v.detail = format!("{store} sent this payment back.");
            v.actions.extend(ret(format!("Return to {store}"), facts.return_url.clone().unwrap_or_default()));
        }
    }
    // A frame that refreshed from an open state sits in a page that still shows the address. The
    // frame cannot reload that page without a script, so it links to the result.
    if !seen.is_empty() && matches!(p.status, PaymentStatus::Finished | PaymentStatus::Failed | PaymentStatus::Refunded) {
        let primary = v.actions.is_empty();
        v.actions.push(Action { label: "Done".into(), href: app.path(&format!("/pay/{}", p.token)), primary });
    }
    v
}

fn ratio(a: Decimal, b: Decimal) -> f64 {
    if b <= Decimal::ZERO { 0.0 } else { (a / b).to_f64().unwrap_or(0.0) }
}

/// Facts of a payment the frame needs beyond the payment row.
pub async fn payment_facts(app: &App, p: &PaymentRow) -> Result<PaymentFacts, Error> {
    let (status_changed_at, return_url) = app.store.payment_checkout_facts(p.payment_id).await?;
    let mut invoice_href = String::new();
    if let Some(id) = p.invoice_id
        && let Some(inv) = app.store.invoice(p.project_id, id).await?
        && inv.expires_at > Utc::now()
    {
        invoice_href = app.path(&format!("/i/{}", inv.token));
    }
    Ok(PaymentFacts { status_changed_at, return_url, invoice_href })
}

async fn status_frame(State(app): State<App>, headers: HeaderMap, Path(token): Path<String>, Query(q): Query<SeenQuery>) -> PageResult {
    let p = app.store.payment_by_token(&token).await?.ok_or(Error::NotFound)?;
    check_host(&app, &headers, p.project_id).await?;
    let an = app.store.asset_network(p.asset_network_id).await?;
    let look = Look::load(&app, p.project_id).await?;
    let facts = payment_facts(&app, &p).await?;
    Ok(render(&payment_frame(&app, look, &p, &an, &facts, &q.seen)))
}

// ---- exchange ------------------------------------------------------------------------

struct CoinChoice {
    key: String,
    href: String,
    name: String,
    ticker: String,
    network: String,
    glyph: String,
}

#[derive(Template)]
#[template(path = "checkout/exchange.html")]
struct ExchangePage {
    look: Look,
    base: String,
    title: String,
    token: String,
    amount: String,
    symbol: String,
    network: String,
    query: String,
    coins: Vec<CoinChoice>,
    more: usize,
    chosen: Option<CoinChoice>,
    chosen_memo: bool,
    refund: String,
    refund_memo: String,
    min_kyc: String,
    error: String,
    open_trade: String,
    closed: bool,
}

/// Coins customers look for first.
const POPULAR: [&str; 14] = ["btc", "eth", "xmr", "usdt", "usdc", "ltc", "sol", "trx", "bnb", "doge", "ton", "bch", "xrp", "dash"];

fn choice(c: &ExchangeCoin, page: &str, query: &str) -> CoinChoice {
    let key = format!("{}:{}", c.ticker, c.network);
    let q = if query.is_empty() { String::new() } else { format!("&q={}", enc(query)) };
    CoinChoice {
        href: format!("{page}?coin={}{q}", enc(&key)),
        key,
        name: c.name.clone(),
        ticker: c.ticker.to_ascii_uppercase(),
        network: if c.network.eq_ignore_ascii_case("mainnet") { String::new() } else { c.network.clone() },
        glyph: glyphs::coin(&c.ticker).unwrap_or_default(),
    }
}

#[derive(Deserialize, Default)]
struct PickQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    coin: String,
}

#[derive(Deserialize, Default)]
struct SwapForm {
    #[serde(default)]
    coin: String,
    #[serde(default)]
    refund: String,
    #[serde(default)]
    refund_memo: String,
}

async fn find_coin(app: &App, key: &str) -> Result<Option<ExchangeCoin>, Error> {
    match key.split_once(':') {
        Some((t, n)) => app.store.exchange_coin(t, n).await,
        None => Ok(None),
    }
}

async fn exchange_view(app: &App, p: &PaymentRow, q: &PickQuery, form: &SwapForm, error: &str) -> PageResult {
    let an = app.store.asset_network(p.asset_network_id).await?;
    let look = Look::load(app, p.project_id).await?;
    let settings = app.store.exchange_settings(p.project_id).await?;
    let available = exchange_offer(app, p, &an).await?;
    let target = exchange::target_coin(app, &an).await?;
    let query = q.q.trim().to_ascii_lowercase();
    let mut list: Vec<ExchangeCoin> = if available {
        app.store
            .exchange_coins()
            .await?
            .into_iter()
            .filter(|c| exchange::allowed(&settings, c))
            .filter(|c| target.as_ref().is_none_or(|t| !(t.ticker == c.ticker && t.network == c.network)))
            .filter(|c| {
                query.is_empty()
                    || c.name.to_ascii_lowercase().contains(&query)
                    || c.ticker.contains(&query)
                    || c.network.to_ascii_lowercase() == query
            })
            .collect()
    } else {
        vec![]
    };
    let rank = |c: &ExchangeCoin| POPULAR.iter().position(|t| *t == c.ticker).unwrap_or(POPULAR.len());
    list.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.name.cmp(&b.name)).then(a.network.cmp(&b.network)));
    const SHOW: usize = 36;
    let more = list.len().saturating_sub(SHOW);
    let chosen = match available {
        true => find_coin(app, if form.coin.is_empty() { &q.coin } else { &form.coin }).await?.filter(|c| exchange::allowed(&settings, c)),
        false => None,
    };
    let open_trade = app
        .store
        .trades_for_payment(p.payment_id)
        .await?
        .into_iter()
        .find(|t| exchange::OPEN_STATUSES.contains(&t.status.as_str()))
        .map(|t| app.path(&format!("/x/{}", t.token)))
        .unwrap_or_default();
    let here = app.path(&format!("/x/{}", p.token));
    let page = ExchangePage {
        title: format!("Pay {} with another coin", look.store),
        look,
        base: app.config.base_path().to_string(),
        token: p.token.clone(),
        amount: fmt_amount(if available { exchange::amount_due(p) } else { p.pay_amount }),
        symbol: an.asset.symbol.clone(),
        network: network_label(&an),
        query: q.q.trim().to_string(),
        coins: list.iter().take(SHOW).map(|c| choice(c, &here, q.q.trim())).collect(),
        more,
        chosen_memo: chosen.as_ref().is_some_and(|c| c.memo),
        chosen: chosen.as_ref().map(|c| choice(c, &here, "")),
        refund: form.refund.clone(),
        refund_memo: form.refund_memo.clone(),
        min_kyc: settings.min_kyc,
        error: error.to_string(),
        open_trade,
        closed: !available,
    };
    Ok(render(&page))
}

/// `/x/{token}`: a payment token shows the coin list, a swap token shows the swap.
async fn exchange_page(State(app): State<App>, headers: HeaderMap, Path(token): Path<String>, Query(q): Query<PickQuery>) -> PageResult {
    if let Some(t) = app.store.trade_by_token(&token).await? {
        check_host(&app, &headers, t.project_id).await?;
        return trade_page(&app, &t).await;
    }
    let p = app.store.payment_by_token(&token).await?.ok_or(Error::NotFound)?;
    check_host(&app, &headers, p.project_id).await?;
    exchange_view(&app, &p, &q, &SwapForm::default(), "").await
}

async fn start_swap(State(app): State<App>, headers: HeaderMap, Path(token): Path<String>, Form(f): Form<SwapForm>) -> PageResult {
    let p = app.store.payment_by_token(&token).await?.ok_or(Error::NotFound)?;
    check_host(&app, &headers, p.project_id).await?;
    let q = PickQuery { q: String::new(), coin: f.coin.clone() };
    let Some(coin) = find_coin(&app, &f.coin).await? else {
        return exchange_view(&app, &p, &q, &f, "Choose a coin.").await;
    };
    let refund = f.refund.trim();
    let refund_memo = f.refund_memo.trim();
    if refund.len() > 200 || refund_memo.len() > 100 || refund.chars().any(char::is_whitespace) {
        return exchange_view(&app, &p, &q, &f, "The refund address is not valid.").await;
    }
    if coin.memo && refund.is_empty() {
        return exchange_view(&app, &p, &q, &f, &format!("{} needs a refund address.", coin.name)).await;
    }
    match exchange::start_trade(&app, &p, &coin, Some(refund), Some(refund_memo)).await {
        Ok(t) => Ok(Redirect::to(&app.path(&format!("/x/{}", t.token))).into_response()),
        Err(e) => {
            e.log();
            exchange_view(&app, &p, &q, &f, &e.public_message()).await
        }
    }
}

#[derive(Template)]
#[template(path = "checkout/trade.html")]
struct TradePage {
    look: Look,
    base: String,
    title: String,
    amount: String,
    ticker: String,
    network: String,
    glyph: String,
    address: String,
    memo: String,
    qr: String,
    provider: String,
    kyc: String,
    rate: String,
    expires: String,
    pays: String,
    trade_id: String,
    frame_src: String,
    payment_href: String,
    retry_href: String,
    done: bool,
}

/// The exchange's network name for customers: "Mainnet" means the coin's own chain.
fn swap_network(coin: &str, network: &str) -> String {
    if network.eq_ignore_ascii_case("mainnet") { coin.to_string() } else { network.to_string() }
}

async fn trade_page(app: &App, t: &TradeRow) -> PageResult {
    let p = app.store.payment_any(t.payment_id).await?.ok_or(Error::NotFound)?;
    let an = app.store.asset_network(p.asset_network_id).await?;
    let look = Look::load(app, p.project_id).await?;
    let rate = if t.amount_from > Decimal::ZERO { sig(t.amount_to / t.amount_from, 6) } else { String::new() };
    let page = TradePage {
        title: format!("Send {} {}", fmt_amount(t.amount_from), t.ticker_from.to_ascii_uppercase()),
        pays: format!("{} {} to {}", fmt_amount(t.amount_to), an.asset.symbol, look.store),
        look,
        base: app.config.base_path().to_string(),
        amount: fmt_amount(t.amount_from),
        ticker: t.ticker_from.to_ascii_uppercase(),
        network: swap_network(&t.coin_from, &t.network_from),
        glyph: glyphs::coin(&t.ticker_from).unwrap_or_default(),
        address: t.address_provider.clone(),
        memo: t.address_provider_memo.clone().unwrap_or_default(),
        qr: qr_svg(&t.address_provider),
        provider: t.provider.clone(),
        kyc: t.kyc_rating.clone().unwrap_or_default(),
        rate: if rate.is_empty() { rate } else { format!("1 {} = {rate} {}", t.ticker_from.to_ascii_uppercase(), an.asset.symbol) },
        expires: t.expires_at.map(|e| e.format("%H:%M UTC").to_string()).unwrap_or_default(),
        trade_id: t.trade_id.clone(),
        frame_src: app.path(&format!("/x/{}/status", t.token)),
        payment_href: app.path(&format!("/pay/{}", p.token)),
        retry_href: app.path(&format!("/x/{}", p.token)),
        done: p.status.is_terminal()
            || matches!(p.status, PaymentStatus::Confirming | PaymentStatus::Confirmed | PaymentStatus::Sending)
            || !exchange::OPEN_STATUSES.contains(&t.status.as_str()),
    };
    Ok(render(&page))
}

/// Status frame contents for a swap. The payment's own state wins once coins arrive.
pub fn trade_frame_view(
    app: &App,
    look: Look,
    t: &TradeRow,
    p: &PaymentRow,
    an: &AssetNetwork,
    facts: &PaymentFacts,
    seen: &str,
) -> FrameView {
    let paid_side = matches!(
        p.status,
        PaymentStatus::Confirming
            | PaymentStatus::Confirmed
            | PaymentStatus::Sending
            | PaymentStatus::Finished
            | PaymentStatus::Refunded
            | PaymentStatus::Failed
    ) || (p.status == PaymentStatus::PartiallyPaid && t.status == "finished");
    if paid_side {
        let mut v = payment_frame(app, look, p, an, facts, seen.strip_prefix("p.").unwrap_or(""));
        if !v.refresh_url.is_empty() {
            let key = v.refresh_url.split("seen=").nth(1).unwrap_or("").to_string();
            v.refresh_url = app.path(&format!("/x/{}/status?seen=p.{key}", t.token));
        }
        return v;
    }
    let store = look.store.clone();
    let coin = t.ticker_from.to_ascii_uppercase();
    let key = state_key(&["swap", &t.status]);
    let fresh = seen != key;
    let open = exchange::OPEN_STATUSES.contains(&t.status.as_str()) && p.status != PaymentStatus::Expired;
    let step = match t.status.as_str() {
        "confirming" => 1,
        "sending" => 2,
        "finished" => 3,
        _ => 0,
    };
    let old_step = if fresh {
        seen.strip_prefix("swap-")
            .map(|s| match s {
                "confirming" => 1,
                "sending" => 2,
                "finished" => 3,
                _ => 0,
            })
            .unwrap_or(0)
    } else {
        step
    };
    let mut v = FrameView {
        look,
        base: app.config.base_path().to_string(),
        kind: "waiting",
        fresh,
        title: String::new(),
        detail: String::new(),
        refresh_url: if open || t.status == "finished" {
            app.path(&format!("/x/{}/status?seen={}", t.token, enc(&key)))
        } else {
            String::new()
        },
        phase: phase_style(),
        cells: grid(cells_for(step as f64 / 4.0), cells_for(old_step as f64 / 4.0)),
        segs: segments(step, 4, old_step),
        progress: format!("Step {} of 4", step + 1),
        countdown: None,
        eta: matches!(t.status.as_str(), "confirming" | "sending").then(|| Eta::new(15 * 60, t.status_changed_at)),
        actions: vec![],
    };
    let retry = Action { label: "Try again".into(), href: app.path(&format!("/x/{}", p.token)), primary: true };
    let direct = Action { label: format!("Pay in {}", an.asset.symbol), href: app.path(&format!("/pay/{}", p.token)), primary: false };
    match t.status.as_str() {
        "confirming" => {
            v.kind = "confirming";
            v.title = format!("The exchange received your {coin}");
            v.detail = "It waits for network confirmations.".into();
        }
        "sending" => {
            v.kind = "confirming";
            v.title = "The exchange is sending".into();
            v.detail = format!("{} {} is on its way to {store}.", fmt_amount(t.amount_to), an.asset.symbol);
        }
        "finished" => {
            v.kind = "confirmed";
            v.title = "The exchange sent the coins".into();
            v.detail = format!("Waiting for them to arrive on {}.", network_name(an));
        }
        "failed" | "halted" => {
            v.kind = "failed";
            v.title = "The swap failed".into();
            v.detail = format!("Contact {} with swap ID {}.", t.provider, t.trade_id);
            v.segs = vec![];
            v.progress = String::new();
            v.actions = vec![retry, direct];
        }
        "expired" => {
            v.kind = "expired";
            v.title = "The swap expired".into();
            v.detail = format!("No {coin} arrived in time. Do not send to this address.");
            v.segs = vec![];
            v.progress = String::new();
            v.actions = vec![retry, direct];
        }
        "refunded" => {
            v.kind = "refunded";
            v.title = "The swap was refunded".into();
            v.detail = format!("{} says it sent your {coin} back.", t.provider);
            v.segs = vec![];
            v.progress = String::new();
            v.actions = vec![retry, direct];
        }
        _ if p.status == PaymentStatus::Expired => {
            v.kind = "expired";
            v.title = "Expired".into();
            v.detail = "The payment window closed. Do not send funds.".into();
            v.segs = vec![];
            v.progress = String::new();
        }
        _ => {
            v.title = format!("Waiting for your {coin}");
            v.detail = format!("Send {} {coin} to the exchange address below.", fmt_amount(t.amount_from));
            if let Some(end) = t.expires_at {
                v.countdown = Countdown::new(Utc::now(), t.created_at, end);
            }
        }
    }
    v
}

async fn trade_frame(State(app): State<App>, headers: HeaderMap, Path(token): Path<String>, Query(q): Query<SeenQuery>) -> PageResult {
    let t = app.store.trade_by_token(&token).await?.ok_or(Error::NotFound)?;
    check_host(&app, &headers, t.project_id).await?;
    let p = app.store.payment_any(t.payment_id).await?.ok_or(Error::NotFound)?;
    let an = app.store.asset_network(p.asset_network_id).await?;
    let look = Look::load(&app, p.project_id).await?;
    let facts = payment_facts(&app, &p).await?;
    Ok(render(&trade_frame_view(&app, look, &t, &p, &an, &facts, &q.seen)))
}

/// The exchange calls this when a swap changes. The body is not trusted: the swap is
/// fetched again with the project's key.
async fn trade_hook(State(app): State<App>, Path(secret): Path<String>, headers: HeaderMap) -> Response {
    let Ok(Some(t)) = app.store.trade_by_hook_secret(&secret).await else {
        return (StatusCode::NOT_FOUND, "Not found.").into_response();
    };
    let key = headers.get("webhook-key").and_then(|v| v.to_str().ok()).unwrap_or("");
    if !crate::crypto::ct_eq(key.as_bytes(), t.webhook_key.as_bytes()) {
        return (StatusCode::UNAUTHORIZED, "Wrong webhook key.").into_response();
    }
    tokio::spawn(async move {
        if let Err(e) = exchange::refresh_trade(&app, &t).await {
            tracing::warn!(trade = %t.trade_id, error = %e, "swap refresh after webhook failed");
        }
    });
    (StatusCode::OK, "ok").into_response()
}

// ---- logo ----------------------------------------------------------------------------

async fn logo(State(app): State<App>, Path((project_id, file)): Path<(i64, String)>) -> Response {
    let hash = file.strip_prefix("logo-").and_then(|f| f.strip_suffix(".png")).unwrap_or("");
    if hash.len() != 16 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return (StatusCode::NOT_FOUND, "Not found.").into_response();
    }
    match app.store.logo_png(project_id, hash).await {
        Ok(Some(png)) => {
            let mut r = png.into_response();
            let h = r.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=31536000, immutable"));
            h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; img-src 'self'"));
            r
        }
        Ok(None) => (StatusCode::NOT_FOUND, "Not found.").into_response(),
        Err(e) => e.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_fills_outward() {
        let cells = grid(8, 3);
        assert_eq!(cells.len(), 49);
        assert_eq!(cells.iter().filter(|c| c.class == "t").count(), 5);
        assert_eq!(cells.iter().filter(|c| c.class.starts_with('f')).count(), 8);
        assert_eq!(cells.iter().filter(|c| c.class == "f n").count(), 5);
        // The first ring fills before the corners.
        assert!(cells[0].class.is_empty());
        assert_eq!(cells_for(1.0), GRID_FREE);
    }

    #[test]
    fn fiat_and_atomic() {
        assert_eq!(fmt_fiat(Decimal::new(123456789, 2), "usd"), "1,234,567.89 USD");
        assert_eq!(fmt_fiat(Decimal::new(5, 0), "eur"), "5.00 EUR");
        assert_eq!(atomic(Decimal::new(15, 1), 18), "1500000000000000000");
        assert_eq!(atomic(Decimal::new(25, 1), 6), "2500000");
        assert_eq!(seen_count("confirming-2", "confirming"), Some(2));
        assert_eq!(seen_count("waiting", "confirming"), None);
    }
}
