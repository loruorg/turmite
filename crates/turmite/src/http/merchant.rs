//! Merchant API under `/v1`. Paths, fields and statuses follow the common hosted-gateway API,
//! so an existing integration only changes its base URL and API key.

use axum::{
    Json, Router,
    extract::{FromRequest, FromRequestParts, Path, Query, Request, State},
    http::{HeaderMap, StatusCode, request::Parts},
    routing::{get, post},
};
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{
    app::App,
    db::{Page, PaymentFilter},
    error::{Error, Result},
    payments::{self, CreateInvoice, CreatePayment, PaymentShape, num},
    rates,
};

pub fn routes() -> Router<App> {
    Router::new()
        .route("/v1/status", get(status))
        .route("/v1/auth", post(auth))
        .route("/v1/currencies", get(currencies))
        .route("/v1/full-currencies", get(full_currencies))
        .route("/v1/merchant/coins", get(merchant_coins))
        .route("/v1/min-amount", get(min_amount))
        .route("/v1/estimate", get(estimate))
        .route("/v1/payment", post(create_payment).get(list_payments))
        .route("/v1/payment/", get(list_payments))
        .route("/v1/payment/{payment_id}", get(get_payment))
        .route("/v1/invoice", post(create_invoice))
        .route("/v1/invoice-payment", post(invoice_payment))
        .route("/v1/balance", get(balance))
        .route("/v1/payout/validate-address", post(validate_address))
}

/// The project of the request's `x-api-key`.
pub struct ApiKey {
    pub project_id: i64,
    pub api_key_id: i64,
}

impl FromRequestParts<App> for ApiKey {
    type Rejection = Error;
    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self> {
        let token = parts
            .headers
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| Error::Unauthorized("Send your API key in the x-api-key header.".into()))?;
        let (project_id, api_key_id) = app.store.verify_api_key(token).await?;
        Ok(ApiKey { project_id, api_key_id })
    }
}

/// An optional API key: public endpoints answer for the key's project when one is sent.
pub struct MaybeApiKey(pub Option<ApiKey>);

impl FromRequestParts<App> for MaybeApiKey {
    type Rejection = Error;
    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self> {
        if parts.headers.get("x-api-key").is_none() {
            return Ok(MaybeApiKey(None));
        }
        ApiKey::from_request_parts(parts, app).await.map(|k| MaybeApiKey(Some(k)))
    }
}

/// A JSON request body. A body that does not parse gives the API error JSON (400), as other errors do.
pub struct Body<T>(pub T);

impl<T: DeserializeOwned + Send, S: Send + Sync> FromRequest<S> for Body<T> {
    type Rejection = Error;
    async fn from_request(req: Request, state: &S) -> Result<Self> {
        let Json(v) = Json::<T>::from_request(req, state).await.map_err(|e| Error::BadRequest(e.body_text()))?;
        Ok(Body(v))
    }
}

/// Query parameters. Parameters that do not parse give the API error JSON (400).
pub struct Params<T>(pub T);

impl<T: DeserializeOwned + Send, S: Send + Sync> FromRequestParts<S> for Params<T> {
    type Rejection = Error;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self> {
        let Query(v) = Query::<T>::from_request_parts(parts, state).await.map_err(|e| Error::BadRequest(e.body_text()))?;
        Ok(Params(v))
    }
}

/// Accept amounts as JSON numbers or strings.
pub fn de_decimal<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Decimal, D::Error> {
    use serde::de::Error as _;
    match Value::deserialize(d)? {
        Value::String(s) => s.trim().parse().map_err(D::Error::custom),
        Value::Number(n) => n.to_string().parse().map_err(D::Error::custom),
        _ => Err(D::Error::custom("expected a number")),
    }
}

pub fn de_opt_decimal<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Option<Decimal>, D::Error> {
    use serde::de::Error as _;
    match Option::<Value>::deserialize(d)? {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => s.trim().parse().map(Some).map_err(D::Error::custom),
        Some(Value::Number(n)) => n.to_string().parse().map(Some).map_err(D::Error::custom),
        _ => Err(D::Error::custom("expected a number")),
    }
}

/// Accept ids as JSON numbers or strings.
pub fn de_opt_string<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Option<String>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s),
        Some(other) => Some(other.to_string()),
    })
}

async fn status() -> Json<Value> {
    Json(json!({ "message": "OK" }))
}

#[derive(Deserialize)]
struct AuthBody {
    email: String,
    password: String,
}

/// Short-lived token for the payout endpoints. The user signs in with their dashboard login.
async fn auth(State(app): State<App>, ip: crate::http::middleware::ClientIp, Body(body): Body<AuthBody>) -> Result<Json<Value>> {
    if !app.limits.login.hit(&format!("ip:{}", ip.key())) || !app.limits.login.hit(&format!("email:{}", body.email.to_lowercase())) {
        return Err(Error::TooManyRequests);
    }
    let user = app.store.user_by_email(&body.email).await?;
    let hash = user.as_ref().map(|u| u.password_hash.clone()).unwrap_or_else(|| crate::auth::DUMMY_HASH.clone());
    let ok = crate::auth::verify_password(&body.password, &hash).await;
    let user = match (ok, user) {
        (true, Some(u)) if u.disabled_at.is_none() => u,
        _ => return Err(Error::Unauthorized("Wrong email or password.".into())),
    };
    let token = crate::auth::jwt::issue(&app.config.secret, user.user_id, Duration::minutes(5))?;
    Ok(Json(json!({ "token": token })))
}

async fn currencies(State(app): State<App>, MaybeApiKey(key): MaybeApiKey) -> Result<Json<Value>> {
    let ids = crate::payments::offered_asset_network_ids(&app, key.map(|k| k.project_id)).await?;
    let list: Vec<String> =
        app.store.list_asset_networks().await?.into_iter().filter(|an| ids.contains(&an.asset_network_id)).map(|an| an.ticker).collect();
    Ok(Json(json!({ "currencies": list })))
}

async fn full_currencies(State(app): State<App>, MaybeApiKey(key): MaybeApiKey) -> Result<Json<Value>> {
    let ids = crate::payments::offered_asset_network_ids(&app, key.map(|k| k.project_id)).await?;
    let items: Vec<Value> = app
        .store
        .list_asset_networks()
        .await?
        .into_iter()
        .filter(|an| an.is_enabled && an.network.is_enabled)
        .map(|an| {
            json!({
                "id": an.asset_network_id,
                "code": an.ticker.to_ascii_uppercase(),
                "name": an.asset.name,
                "enable": ids.contains(&an.asset_network_id),
                "wallet_regex": Value::Null,
                "priority": an.asset_network_id,
                "extra_id_exists": an.network.kind == crate::domain::NetworkKind::AccountMemo,
                "extra_id_regex": Value::Null,
                "logo_url": Value::Null,
                "track": false,
                "cg_id": an.asset.coingecko_id,
                "is_maxlimit": false,
                "network": an.network.api_name(),
                "smart_contract": an.contract_address,
                "network_precision": an.decimals.to_string(),
                "ticker": an.ticker,
            })
        })
        .collect();
    Ok(Json(json!({ "currencies": items })))
}

async fn merchant_coins(State(app): State<App>, key: ApiKey) -> Result<Json<Value>> {
    let ids = crate::payments::offered_asset_network_ids(&app, Some(key.project_id)).await?;
    let list: Vec<String> =
        app.store.list_asset_networks().await?.into_iter().filter(|an| ids.contains(&an.asset_network_id)).map(|an| an.ticker).collect();
    Ok(Json(json!({ "selectedCurrencies": list })))
}

#[derive(Deserialize)]
struct MinAmountQuery {
    currency_from: String,
    currency_to: Option<String>,
    fiat_equivalent: Option<String>,
}

async fn min_amount(State(app): State<App>, Params(q): Params<MinAmountQuery>) -> Result<Json<Value>> {
    let an = app.store.asset_network_by_ticker(&q.currency_from).await?;
    let min = payments::min_payment_amount(&an);
    let mut out = json!({
        "currency_from": an.ticker,
        "currency_to": q.currency_to.unwrap_or_else(|| an.ticker.clone()),
        "min_amount": num(min),
    });
    if let (Some(fiat), Some(cg)) = (q.fiat_equivalent.as_deref(), an.asset.coingecko_id.as_deref())
        && let Ok(price) = app.rates.price(cg, &fiat.to_ascii_lowercase()).await
    {
        out["fiat_equivalent"] = num((min * price).round_dp(2));
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
struct EstimateQuery {
    amount: String,
    currency_from: String,
    currency_to: String,
}

async fn estimate(State(app): State<App>, Params(q): Params<EstimateQuery>) -> Result<Json<Value>> {
    let amount: Decimal = q.amount.trim().parse().map_err(|_| Error::BadRequest("`amount` is not a number.".into()))?;
    let an = app.store.asset_network_by_ticker(&q.currency_to).await?;
    let cg = an.asset.coingecko_id.as_deref().ok_or_else(|| Error::BadRequest("This currency has no price source.".into()))?;
    let (est, _) = rates::estimate(app.rates.as_ref(), amount, &q.currency_from, cg, an.decimals as u32).await?;
    Ok(Json(json!({
        "currency_from": q.currency_from.to_ascii_lowercase(),
        "amount_from": num(amount),
        "currency_to": an.ticker,
        "estimated_amount": est.normalize().to_string(),
    })))
}

#[derive(Deserialize)]
struct CreatePaymentBody {
    #[serde(deserialize_with = "de_decimal")]
    price_amount: Decimal,
    price_currency: String,
    pay_currency: String,
    #[serde(default, deserialize_with = "de_opt_decimal")]
    pay_amount: Option<Decimal>,
    #[serde(default, deserialize_with = "de_opt_string")]
    order_id: Option<String>,
    order_description: Option<String>,
    ipn_callback_url: Option<String>,
    success_url: Option<String>,
    cancel_url: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string")]
    purchase_id: Option<String>,
    idempotency_key: Option<String>,
}

/// A created resource: 201, as the common API answers.
type Created = (StatusCode, Json<Value>);

async fn create_payment(State(app): State<App>, key: ApiKey, headers: HeaderMap, Body(b): Body<CreatePaymentBody>) -> Result<Created> {
    let idempotency_key = b.idempotency_key.or_else(|| headers.get("idempotency-key").and_then(|v| v.to_str().ok()).map(str::to_string));
    let p = payments::create_payment(
        &app,
        key.project_id,
        CreatePayment {
            price_amount: b.price_amount,
            price_currency: b.price_currency,
            pay_currency: b.pay_currency,
            pay_amount: b.pay_amount,
            order_id: b.order_id,
            order_description: b.order_description,
            ipn_callback_url: b.ipn_callback_url,
            success_url: b.success_url,
            cancel_url: b.cancel_url,
            purchase_id: b.purchase_id,
            idempotency_key,
            invoice_id: None,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(payments::payment_json_full(&app, &p, PaymentShape::Created).await?)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    limit: Option<i64>,
    page: Option<i64>,
    sort_by: Option<String>,
    order_by: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    #[serde(rename = "payment_status")]
    status: Option<String>,
    #[serde(rename = "order_id")]
    order_id: Option<String>,
    invoice_id: Option<String>,
}

fn parse_date(s: Option<&str>, end_of_day: bool) -> Result<Option<DateTime<Utc>>> {
    let Some(s) = s.map(str::trim).filter(|s| !s.is_empty()) else { return Ok(None) };
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(Some(dt.with_timezone(&Utc)));
    }
    let d = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| Error::BadRequest(format!("Bad date `{s}`.")))?;
    let t = if end_of_day { d.and_hms_opt(23, 59, 59) } else { d.and_hms_opt(0, 0, 0) };
    Ok(t.map(|t| t.and_utc()))
}

async fn list_payments(State(app): State<App>, key: ApiKey, Params(q): Params<ListQuery>) -> Result<Json<Value>> {
    let limit = q.limit.unwrap_or(10).clamp(1, 500);
    let page = q.page.unwrap_or(0).max(0);
    let filter = PaymentFilter {
        status: q.status.filter(|s| !s.is_empty()),
        order_id: q.order_id.filter(|s| !s.is_empty()),
        invoice_id: match q.invoice_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => Some(s.parse().map_err(|_| Error::BadRequest("`invoiceId` is not a number.".into()))?),
            None => None,
        },
        date_from: parse_date(q.date_from.as_deref(), false)?,
        date_to: parse_date(q.date_to.as_deref(), true)?,
        sort_by: q.sort_by,
        ascending: q.order_by.as_deref() == Some("asc"),
    };
    let (rows, total) = app.store.list_payments(key.project_id, &filter, Page::new(limit, page)).await?;
    let mut data = Vec::with_capacity(rows.len());
    for p in &rows {
        data.push(payments::payment_json_full(&app, p, PaymentShape::Status).await?);
    }
    Ok(Json(json!({
        "data": data,
        "limit": limit,
        "page": page,
        "pagesCount": (total + limit - 1) / limit,
        "total": total,
    })))
}

async fn get_payment(State(app): State<App>, key: ApiKey, Path(payment_id): Path<String>) -> Result<Json<Value>> {
    let id: i64 = payment_id.trim().parse().map_err(|_| Error::NotFound)?;
    let p = app.store.payment(key.project_id, id).await?.ok_or(Error::NotFound)?;
    Ok(Json(payments::payment_json_full(&app, &p, PaymentShape::Status).await?))
}

#[derive(Deserialize)]
struct CreateInvoiceBody {
    #[serde(deserialize_with = "de_decimal")]
    price_amount: Decimal,
    price_currency: String,
    pay_currency: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string")]
    order_id: Option<String>,
    order_description: Option<String>,
    ipn_callback_url: Option<String>,
    success_url: Option<String>,
    cancel_url: Option<String>,
}

async fn create_invoice(State(app): State<App>, key: ApiKey, Body(b): Body<CreateInvoiceBody>) -> Result<Created> {
    let invoice = payments::create_invoice(
        &app,
        key.project_id,
        CreateInvoice {
            price_amount: b.price_amount,
            price_currency: b.price_currency,
            pay_currency: b.pay_currency,
            order_id: b.order_id,
            order_description: b.order_description,
            ipn_callback_url: b.ipn_callback_url,
            success_url: b.success_url,
            cancel_url: b.cancel_url,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(payments::invoice_json(&app, &invoice).await?)))
}

#[derive(Deserialize)]
struct InvoicePaymentBody {
    #[serde(deserialize_with = "de_opt_string")]
    iid: Option<String>,
    pay_currency: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    purchase_id: Option<String>,
}

async fn invoice_payment(State(app): State<App>, key: ApiKey, Body(b): Body<InvoicePaymentBody>) -> Result<Created> {
    let iid: i64 = b.iid.as_deref().and_then(|s| s.trim().parse().ok()).ok_or_else(|| Error::BadRequest("`iid` is required.".into()))?;
    let invoice = app.store.invoice(key.project_id, iid).await?.ok_or(Error::NotFound)?;
    let p = payments::pay_invoice(&app, &invoice, &b.pay_currency, b.purchase_id).await?;
    Ok((StatusCode::CREATED, Json(payments::payment_json_full(&app, &p, PaymentShape::Created).await?)))
}

async fn balance(State(app): State<App>, key: ApiKey) -> Result<Json<Value>> {
    let mut map = serde_json::Map::new();
    for (ticker, amount, pending) in app.store.balances(key.project_id).await? {
        map.insert(ticker, json!({ "amount": num(amount), "pendingAmount": num(pending) }));
    }
    Ok(Json(Value::Object(map)))
}

#[derive(Deserialize)]
struct ValidateAddressBody {
    currency: String,
    address: String,
}

async fn validate_address(State(app): State<App>, _key: ApiKey, Body(b): Body<ValidateAddressBody>) -> Result<Json<Value>> {
    let an = app.store.asset_network_by_ticker(&b.currency).await?;
    if !crate::addresses::validate(&app, &an.network, b.address.trim()).await? {
        return Err(Error::BadRequest(format!("Address is not valid for {}.", an.ticker)));
    }
    Ok(Json(json!("OK")))
}
