//! Creating payments and invoices, and their public JSON shape.

use chrono::{Duration, SecondsFormat, Utc};
use rust_decimal::{Decimal, prelude::ToPrimitive};
use serde_json::{Value, json};

use crate::{
    app::App,
    db::{InvoiceRow, NewPayment, PaymentRow},
    domain::AssetNetwork,
    error::{Error, Result},
    events::DomainEvent,
    ids, rates,
};

/// A decimal as a JSON number. The API sends numbers, as integrations expect.
/// Whole amounts are written as integers (`100`, not `100.0`), so integrations that parse and
/// re-serialize the body compute the same signature.
pub fn num(d: Decimal) -> Value {
    let d = d.normalize();
    if d.scale() == 0
        && let Some(i) = d.to_i64()
    {
        return Value::Number(i.into());
    }
    d.to_f64().and_then(serde_json::Number::from_f64).map(Value::Number).unwrap_or(Value::Null)
}

fn ts(d: chrono::DateTime<Utc>) -> String {
    d.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Extra facts for the payment object that need more queries.
#[derive(Default)]
pub struct PaymentExtras {
    pub payin_hash: Option<String>,
    pub payout_hash: Option<String>,
    /// The project's checkout domain, for `payment_url`.
    pub checkout_domain: Option<String>,
}

/// Where a payment object goes. The common API types two ids differently per place, and typed
/// clients of that API reject the other type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaymentShape {
    /// `POST /v1/payment` and `POST /v1/invoice-payment`: `payment_id` and `purchase_id` are strings.
    Created,
    /// `GET /v1/payment/{id}` and `GET /v1/payment`: both are numbers (`purchase_id` only when numeric).
    Status,
    /// Webhook bodies: `payment_id` is a number and `purchase_id` a string.
    Webhook,
}

/// Largest integer that JavaScript clients parse without loss.
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// `purchase_id` as a JSON number when it is a plain decimal integer, else as a string.
fn purchase_id_value(s: &str) -> Value {
    let plain = !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'));
    match s.parse::<u64>() {
        Ok(n) if plain && n <= MAX_SAFE_INTEGER => Value::Number(n.into()),
        _ => Value::String(s.to_string()),
    }
}

/// The payment object returned by the API and sent in IPNs.
pub fn payment_json(app: &App, p: &PaymentRow, an: &AssetNetwork, extras: &PaymentExtras, shape: PaymentShape) -> Value {
    let (payment_id, purchase_id) = match shape {
        PaymentShape::Created => (Value::String(p.payment_id.to_string()), Value::String(p.purchase_id.clone())),
        PaymentShape::Status => (Value::Number(p.payment_id.into()), purchase_id_value(&p.purchase_id)),
        PaymentShape::Webhook => (Value::Number(p.payment_id.into()), Value::String(p.purchase_id.clone())),
    };
    json!({
        "payment_id": payment_id,
        "invoice_id": p.invoice_id,
        "payment_status": p.status.as_str(),
        "pay_address": p.pay_address,
        "payin_extra_id": p.payin_extra_id,
        "price_amount": num(p.price_amount),
        "price_currency": p.price_currency.to_ascii_lowercase(),
        "pay_amount": num(p.pay_amount),
        "actually_paid": num(p.actually_paid),
        "pay_currency": an.ticker,
        "order_id": p.order_id,
        "order_description": p.order_description,
        "purchase_id": purchase_id,
        "outcome_amount": p.outcome_amount.map(num).unwrap_or(Value::Null),
        "outcome_currency": an.ticker,
        "network": an.network.api_name(),
        "network_precision": an.decimals,
        "smart_contract": an.contract_address,
        "payin_hash": extras.payin_hash,
        "payout_hash": extras.payout_hash,
        "payment_url": crate::http::payment_url(app, extras.checkout_domain.as_deref(), &p.token),
        "created_at": ts(p.created_at),
        "updated_at": ts(p.updated_at),
        "expiration_estimate_date": ts(p.expires_at),
        // Fields of the common API that have no other value in Turmite.
        "parent_payment_id": Value::Null,
        "type": "crypto2crypto",
        "time_limit": Value::Null,
        "burning_percent": Value::Null,
        "payment_extra_ids": Value::Null,
        "amount_received": num(p.outcome_amount.unwrap_or(p.pay_amount)),
        "actually_paid_at_fiat": num(paid_at_fiat(p)),
        // Turmite takes no fee. The only cost is the network fee of the sweep to the merchant's wallet.
        "fee": {
            "currency": an.ticker,
            "depositFee": 0,
            "withdrawalFee": num(p.network_fee_amount),
            "serviceFee": 0,
        },
    })
}

/// `actually_paid` in the price currency at the payment's rate. A payment with a fixed pay
/// amount has no stored rate: then the rate is price / amount due.
fn paid_at_fiat(p: &PaymentRow) -> Decimal {
    let rate = p.rate.or_else(|| (!p.pay_amount.is_zero()).then(|| p.price_amount / p.pay_amount)).unwrap_or(Decimal::ZERO);
    (p.actually_paid * rate).round_dp(8)
}

/// Payment JSON with the extra fields filled from the database.
pub async fn payment_json_full(app: &App, p: &PaymentRow, shape: PaymentShape) -> Result<Value> {
    let an = app.store.asset_network(p.asset_network_id).await?;
    let txs = app.store.payment_transactions(p.payment_id).await?;
    let sweeps = app.store.sweeps_for_payment(p.payment_id).await?;
    let extras = PaymentExtras {
        // The newest deposit that still counts (a dropped one has -1 confirmations).
        payin_hash: txs.iter().rev().find(|t| t.confirmations >= 0).map(|t| t.txid.clone()),
        payout_hash: sweeps.iter().rev().find_map(|s| s.txid.clone()),
        checkout_domain: app.store.checkout_domain(p.project_id).await?,
    };
    Ok(payment_json(app, p, &an, &extras, shape))
}

pub async fn invoice_json(app: &App, i: &InvoiceRow) -> Result<Value> {
    let domain = app.store.checkout_domain(i.project_id).await?;
    Ok(json!({
        "id": i.invoice_id.to_string(),
        "token_id": i.token,
        "order_id": i.order_id,
        "order_description": i.order_description,
        "price_amount": i.price_amount.normalize().to_string(),
        "price_currency": i.price_currency.to_ascii_lowercase(),
        "pay_currency": i.pay_currency,
        "ipn_callback_url": i.ipn_callback_url,
        "invoice_url": crate::http::invoice_url(app, domain.as_deref(), &i.token),
        "success_url": i.success_url,
        "cancel_url": i.cancel_url,
        "created_at": ts(i.created_at),
        // An invoice does not change after it is made.
        "updated_at": ts(i.created_at),
        "expiration_estimate_date": ts(i.expires_at),
    }))
}

/// What a caller asks for when creating a payment.
#[derive(Clone, Debug, Default)]
pub struct CreatePayment {
    pub price_amount: Decimal,
    pub price_currency: String,
    pub pay_currency: String,
    /// Fixed amount in the pay currency. Without it, the amount follows the rate.
    pub pay_amount: Option<Decimal>,
    pub order_id: Option<String>,
    pub order_description: Option<String>,
    pub ipn_callback_url: Option<String>,
    pub success_url: Option<String>,
    pub cancel_url: Option<String>,
    pub purchase_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub invoice_id: Option<i64>,
}

const MAX_TEXT: usize = 2048;

fn check_text(name: &str, v: &Option<String>, max: usize) -> Result<()> {
    if v.as_ref().is_some_and(|s| s.len() > max) {
        return Err(Error::BadRequest(format!("`{name}` is longer than {max} characters.")));
    }
    Ok(())
}

/// `http(s)` URL check for callback and redirect URLs.
pub fn check_url(app: &App, name: &str, v: &Option<String>) -> Result<()> {
    let Some(raw) = v.as_deref().filter(|s| !s.is_empty()) else { return Ok(()) };
    let u = url::Url::parse(raw).map_err(|_| Error::BadRequest(format!("`{name}` is not a valid URL.")))?;
    let ok_scheme = u.scheme() == "https" || (u.scheme() == "http" && !app.config.mode.is_mainnet());
    if !ok_scheme || u.host_str().is_none() || !u.username().is_empty() || u.password().is_some() {
        return Err(Error::BadRequest(format!("`{name}` must be an https:// URL without credentials.")));
    }
    if raw.len() > MAX_TEXT {
        return Err(Error::BadRequest(format!("`{name}` is too long.")));
    }
    Ok(())
}

/// Smallest payment in an asset: the catalog minimum, else a small default from its decimals.
/// `GET /v1/min-amount` reports it and payment creation enforces it.
pub fn min_payment_amount(an: &AssetNetwork) -> Decimal {
    an.min_payment_amount.unwrap_or_else(|| Decimal::new(1, (an.decimals as u32).saturating_sub(2).clamp(1, 8)))
}

/// Enabled pay currency for a project, or a clear error.
pub async fn payable(app: &App, project_id: i64, ticker: &str) -> Result<AssetNetwork> {
    let an = app.store.asset_network_by_ticker(ticker).await?;
    if !app.store.enabled_asset_network_ids(project_id).await?.contains(&an.asset_network_id) {
        return Err(Error::BadRequest(format!(
            "Currency `{}` is not enabled for this project. Add a {} wallet in the dashboard.",
            an.ticker, an.network.name
        )));
    }
    if app.drivers().get(&an.network.chain).is_none() {
        return Err(Error::Unavailable(format!(
            "Currency `{}` is not available now: no {} node is connected.",
            an.ticker, an.network.name
        )));
    }
    Ok(an)
}

/// Asset networks to offer: enabled for the project (any project when `None`), on a
/// network with a connected node.
pub async fn offered_asset_network_ids(app: &App, project_id: Option<i64>) -> Result<Vec<i64>> {
    let ids = match project_id {
        Some(p) => app.store.enabled_asset_network_ids(p).await?,
        None => app.store.enabled_asset_network_ids_any().await?,
    };
    let drivers = app.drivers();
    Ok(app
        .store
        .list_asset_networks()
        .await?
        .into_iter()
        .filter(|an| ids.contains(&an.asset_network_id) && drivers.get(&an.network.chain).is_some())
        .map(|an| an.asset_network_id)
        .collect())
}

pub async fn create_payment(app: &App, project_id: i64, req: CreatePayment) -> Result<PaymentRow> {
    if req.price_amount <= Decimal::ZERO {
        return Err(Error::BadRequest("`price_amount` must be greater than 0.".into()));
    }
    check_text("order_id", &req.order_id, 255)?;
    check_text("order_description", &req.order_description, 1000)?;
    check_text("purchase_id", &req.purchase_id, 255)?;
    check_text("idempotency_key", &req.idempotency_key, 255)?;
    check_url(app, "ipn_callback_url", &req.ipn_callback_url)?;
    check_url(app, "success_url", &req.success_url)?;
    check_url(app, "cancel_url", &req.cancel_url)?;
    if req.invoice_id.is_none() {
        crate::branding::check_return_hosts(app, project_id, &[("success_url", &req.success_url), ("cancel_url", &req.cancel_url)]).await?;
    }

    if let Some(key) = req.idempotency_key.as_deref()
        && let Some(existing) = app.store.payment_by_idempotency_key(project_id, key).await?
    {
        return Ok(existing);
    }

    let an = payable(app, project_id, &req.pay_currency).await?;
    let price_currency = req.price_currency.trim().to_ascii_lowercase();
    if price_currency.is_empty() || price_currency.len() > 12 {
        return Err(Error::BadRequest("`price_currency` is required.".into()));
    }
    let (pay_amount, rate) = match req.pay_amount {
        Some(a) if a > Decimal::ZERO => (a.round_dp(an.decimals as u32), None),
        _ if price_currency == an.ticker || price_currency == an.asset.symbol.to_ascii_lowercase() => {
            (req.price_amount.round_dp(an.decimals as u32), Some(Decimal::ONE))
        }
        _ => {
            let cg = an.asset.coingecko_id.as_deref().ok_or_else(|| Error::BadRequest("This currency has no price source.".into()))?;
            let (amount, price) = rates::estimate(app.rates.as_ref(), req.price_amount, &price_currency, cg, an.decimals as u32).await?;
            (amount, Some(price))
        }
    };
    let min = min_payment_amount(&an);
    if pay_amount < min {
        return Err(Error::BadRequest(format!("The amount is below the minimum of {} {}.", min.normalize(), an.ticker)));
    }

    let project = app.store.project(project_id).await?;
    let alloc = crate::addresses::allocate(app, project_id, &an).await?;
    let purchase_id = req.purchase_id.clone().unwrap_or_else(ids::numeric_id);
    let new = NewPayment {
        project_id,
        invoice_id: req.invoice_id,
        asset_network_id: an.asset_network_id,
        network_id: an.network.network_id,
        deposit_address_id: alloc.deposit_address_id,
        idempotency_key: req.idempotency_key.as_deref(),
        purchase_id: &purchase_id,
        order_id: req.order_id.as_deref(),
        order_description: req.order_description.as_deref(),
        price_amount: req.price_amount,
        price_currency: &price_currency,
        pay_address: &alloc.address,
        payin_extra_id: alloc.payin_extra_id.as_deref(),
        pay_amount,
        rate,
        ipn_callback_url: req.ipn_callback_url.as_deref().filter(|s| !s.is_empty()),
        success_url: req.success_url.as_deref().filter(|s| !s.is_empty()),
        cancel_url: req.cancel_url.as_deref().filter(|s| !s.is_empty()),
        expires_at: Utc::now() + Duration::minutes(project.payment_expiry_minutes as i64),
    };
    let payment = match app.store.create_payment(&new, &ids::token()).await {
        Ok(p) => p,
        Err(e) => {
            // No payment took the address: give it back.
            if let Err(re) = alloc.release(app).await {
                tracing::warn!(deposit_address_id = ?alloc.deposit_address_id, error = %re, "releasing a deposit address failed");
            }
            // A request with the same idempotency key was inserted first: answer with that payment.
            if let (Error::Conflict(_), Some(key)) = (&e, req.idempotency_key.as_deref())
                && let Some(first) = app.store.payment_by_idempotency_key(project_id, key).await?
            {
                return Ok(first);
            }
            return Err(e);
        }
    };
    app.events.publish(DomainEvent::PaymentCreated { payment_id: payment.payment_id });
    crate::webhooks::payment_changed(app, &payment).await?;
    Ok(payment)
}

/// What a caller asks for when creating an invoice.
#[derive(Clone, Debug, Default)]
pub struct CreateInvoice {
    pub price_amount: Decimal,
    pub price_currency: String,
    pub pay_currency: Option<String>,
    pub order_id: Option<String>,
    pub order_description: Option<String>,
    pub ipn_callback_url: Option<String>,
    pub success_url: Option<String>,
    pub cancel_url: Option<String>,
}

pub async fn create_invoice(app: &App, project_id: i64, req: CreateInvoice) -> Result<InvoiceRow> {
    if req.price_amount <= Decimal::ZERO {
        return Err(Error::BadRequest("`price_amount` must be greater than 0.".into()));
    }
    check_text("order_id", &req.order_id, 255)?;
    check_text("order_description", &req.order_description, 1000)?;
    check_url(app, "ipn_callback_url", &req.ipn_callback_url)?;
    check_url(app, "success_url", &req.success_url)?;
    check_url(app, "cancel_url", &req.cancel_url)?;
    crate::branding::check_return_hosts(app, project_id, &[("success_url", &req.success_url), ("cancel_url", &req.cancel_url)]).await?;
    let pay_currency = match req.pay_currency.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(t) => Some(payable(app, project_id, t).await?.ticker),
        None => None,
    };
    let price_currency = req.price_currency.trim().to_ascii_lowercase();
    if price_currency.is_empty() {
        return Err(Error::BadRequest("`price_currency` is required.".into()));
    }
    let project = app.store.project(project_id).await?;
    app.store
        .create_invoice(
            project_id,
            &ids::token(),
            req.order_id.as_deref(),
            req.order_description.as_deref(),
            req.price_amount,
            &price_currency,
            pay_currency.as_deref(),
            req.ipn_callback_url.as_deref().filter(|s| !s.is_empty()),
            req.success_url.as_deref().filter(|s| !s.is_empty()),
            req.cancel_url.as_deref().filter(|s| !s.is_empty()),
            Utc::now() + Duration::minutes(project.invoice_expiry_minutes as i64),
        )
        .await
}

/// Payment for an invoice in `ticker`. Reuses the invoice's open payment in that currency.
pub async fn pay_invoice(app: &App, invoice: &InvoiceRow, ticker: &str, purchase_id: Option<String>) -> Result<PaymentRow> {
    if invoice.expires_at < Utc::now() {
        return Err(Error::BadRequest("This invoice has expired.".into()));
    }
    let an = payable(app, invoice.project_id, ticker).await?;
    if let Some(p) = app.store.latest_payment_for_invoice(invoice.invoice_id).await?
        && p.asset_network_id == an.asset_network_id
        && !p.status.is_terminal()
        && p.expires_at > Utc::now()
    {
        return Ok(p);
    }
    create_payment(
        app,
        invoice.project_id,
        CreatePayment {
            price_amount: invoice.price_amount,
            price_currency: invoice.price_currency.clone(),
            pay_currency: an.ticker,
            order_id: invoice.order_id.clone(),
            order_description: invoice.order_description.clone(),
            ipn_callback_url: invoice.ipn_callback_url.clone(),
            success_url: invoice.success_url.clone(),
            cancel_url: invoice.cancel_url.clone(),
            purchase_id,
            invoice_id: Some(invoice.invoice_id),
            ..Default::default()
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purchase_id_is_a_number_only_when_exact() {
        assert_eq!(purchase_id_value("6084815412"), json!(6084815412u64));
        assert_eq!(purchase_id_value("0"), json!(0));
        assert_eq!(purchase_id_value("0123"), json!("0123"));
        assert_eq!(purchase_id_value("A-1"), json!("A-1"));
        assert_eq!(purchase_id_value(""), json!(""));
        assert_eq!(purchase_id_value("9007199254740991"), json!(9007199254740991u64));
        assert_eq!(purchase_id_value("9007199254740992"), json!("9007199254740992"));
    }

    #[test]
    fn whole_amounts_are_integers() {
        assert_eq!(num("100.00".parse().unwrap()).to_string(), "100");
        assert_eq!(num("0.00123".parse().unwrap()).to_string(), "0.00123");
        // The API reference names this form: very small amounts have an exponent.
        assert_eq!(num("0.000001".parse().unwrap()).to_string(), "1e-6");
    }
}
