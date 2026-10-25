//! Payout endpoints of the merchant API. Creating, verifying and listing need the project API key and a
//! bearer token from `POST /v1/auth`. `POST /v1/payout/validate-address` lives in `merchant.rs`.

use axum::{
    Json, Router,
    extract::{FromRequestParts, Path, State},
    http::{StatusCode, request::Parts},
    routing::{get, post},
};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    app::App,
    db::{Page, PayoutFilter, UserRow},
    error::{Error, Result},
    http::{
        merchant::{ApiKey, Body, Params, de_decimal, de_opt_string},
        middleware::ClientIp,
    },
    payments::num,
    payouts::{
        Catalog, PayoutStatus, batch_json, payout_json,
        service::{self, NewWithdrawal, Source},
    },
};

pub fn routes() -> Router<App> {
    Router::new()
        .route("/v1/payout", post(create).get(list))
        .route("/v1/payout/fee", get(fee))
        .route("/v1/payout/{batch_id}", get(get_batch))
        .route("/v1/payout/{batch_id}/verify", post(verify))
}

/// API key plus the bearer token of a dashboard user who is in the key's project.
pub struct PayoutAuth {
    pub project_id: i64,
    pub api_key_id: i64,
    pub user: UserRow,
    pub is_project_admin: bool,
    pub ip: String,
}

impl FromRequestParts<App> for PayoutAuth {
    type Rejection = Error;
    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self> {
        let key = ApiKey::from_request_parts(parts, app).await?;
        let token = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
            .ok_or_else(|| Error::Unauthorized("Send a token from POST /v1/auth as `Authorization: Bearer <token>`.".into()))?;
        let user_id = crate::auth::jwt::verify(&app.config.secret, token)?;
        let user = app.store.user(user_id).await.map_err(|_| Error::Unauthorized("Unknown user.".into()))?;
        if user.disabled_at.is_some() {
            return Err(Error::Unauthorized("This user is disabled.".into()));
        }
        let role = app.store.member_role(key.project_id, user_id).await?;
        if !user.is_admin() && role.is_none() {
            return Err(Error::Forbidden("This user is not a member of the API key's project.".into()));
        }
        let is_project_admin = user.is_admin() || role.as_deref() == Some("project_admin");
        let ip = ClientIp::from_request_parts(parts, app).await.map(|c| c.key()).unwrap_or_default();
        Ok(Self { project_id: key.project_id, api_key_id: key.api_key_id, user, is_project_admin, ip })
    }
}

impl PayoutAuth {
    fn require_admin(&self) -> Result<()> {
        if self.is_project_admin { Ok(()) } else { Err(Error::Forbidden("Only a project admin can create or verify payouts.".into())) }
    }

    async fn audit(&self, app: &App, action: &str, entity_id: &str, data: Value) {
        if let Err(e) = app
            .store
            .insert_audit(
                Some(self.user.user_id),
                Some(self.project_id),
                "user",
                action,
                "payout_batch",
                Some(entity_id),
                &data,
                Some(&self.ip),
            )
            .await
        {
            tracing::error!(error = %e, action, "audit log write failed");
        }
    }
}

#[derive(Deserialize)]
struct WithdrawalBody {
    address: String,
    currency: String,
    #[serde(deserialize_with = "de_decimal")]
    amount: Decimal,
    #[serde(default, deserialize_with = "de_opt_string")]
    extra_id: Option<String>,
    ipn_callback_url: Option<String>,
    payout_description: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string")]
    unique_external_id: Option<String>,
}

#[derive(Deserialize)]
struct CreateBody {
    withdrawals: Vec<WithdrawalBody>,
    ipn_callback_url: Option<String>,
}

async fn create(State(app): State<App>, auth: PayoutAuth, Body(b): Body<CreateBody>) -> Result<(StatusCode, Json<Value>)> {
    auth.require_admin()?;
    let withdrawals = b
        .withdrawals
        .into_iter()
        .map(|w| NewWithdrawal {
            address: w.address,
            currency: w.currency,
            amount: w.amount,
            extra_id: w.extra_id,
            ipn_callback_url: w.ipn_callback_url,
            description: w.payout_description,
            unique_external_id: w.unique_external_id,
        })
        .collect();
    let source = Source::Api { user_id: auth.user.user_id, api_key_id: auth.api_key_id };
    let out = service::create_batch(&app, auth.project_id, source, withdrawals, b.ipn_callback_url).await?;
    auth.audit(
        &app,
        "payout.create",
        &out.batch_id.to_string(),
        json!({ "api_key_id": auth.api_key_id, "withdrawals": out.payouts.len() }),
    )
    .await;
    let catalog = Catalog::load(&app).await?;
    Ok((StatusCode::CREATED, Json(batch_json(out.batch_id, &out.payouts, &catalog))))
}

#[derive(Deserialize)]
struct VerifyBody {
    #[serde(default, deserialize_with = "de_opt_string")]
    verification_code: Option<String>,
}

fn batch_id(s: &str) -> Result<i64> {
    s.trim().parse().map_err(|_| Error::NotFound)
}

async fn verify(State(app): State<App>, auth: PayoutAuth, Path(id): Path<String>, Body(b): Body<VerifyBody>) -> Result<Json<Value>> {
    auth.require_admin()?;
    let id = batch_id(&id)?;
    if !app.store.batch_exists(auth.project_id, id).await? {
        return Err(Error::NotFound);
    }
    let code = b.verification_code.unwrap_or_default();
    service::check_user_totp(&app, &auth.user, &code).await?;
    let payouts = service::verify_batch(&app, auth.project_id, id, auth.user.user_id).await?;
    auth.audit(&app, "payout.verify", &id.to_string(), json!({})).await;
    let catalog = Catalog::load(&app).await?;
    Ok(Json(batch_json(id, &payouts, &catalog)))
}

async fn get_batch(State(app): State<App>, key: ApiKey, Path(id): Path<String>) -> Result<Json<Value>> {
    let id = batch_id(&id)?;
    if !app.store.batch_exists(key.project_id, id).await? {
        return Err(Error::NotFound);
    }
    let payouts = app.store.batch_payouts(key.project_id, id).await?;
    let catalog = Catalog::load(&app).await?;
    Ok(Json(batch_json(id, &payouts, &catalog)))
}

#[derive(Deserialize)]
struct ListQuery {
    batch_id: Option<String>,
    status: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    order: Option<String>,
    limit: Option<i64>,
    page: Option<i64>,
}

fn parse_date(s: Option<&str>, end_of_day: bool) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    let Some(s) = s.map(str::trim).filter(|s| !s.is_empty()) else { return Ok(None) };
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(Some(dt.with_timezone(&chrono::Utc)));
    }
    let d = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| Error::BadRequest(format!("Bad date `{s}`.")))?;
    let t = if end_of_day { d.and_hms_opt(23, 59, 59) } else { d.and_hms_opt(0, 0, 0) };
    Ok(t.map(|t| t.and_utc()))
}

async fn list(State(app): State<App>, auth: PayoutAuth, Params(q): Params<ListQuery>) -> Result<Json<Value>> {
    let limit = q.limit.unwrap_or(10).clamp(1, 500);
    let page = q.page.unwrap_or(0).max(0);
    let status = q.status.filter(|s| !s.is_empty());
    if let Some(s) = &status
        && PayoutStatus::parse(s).is_none()
    {
        return Err(Error::BadRequest(format!("Unknown status `{s}`.")));
    }
    let filter = PayoutFilter {
        status,
        batch_id: match q.batch_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(b) => Some(b.parse().map_err(|_| Error::BadRequest("`batch_id` is not a number.".into()))?),
            None => None,
        },
        network_id: None,
        date_from: parse_date(q.date_from.as_deref(), false)?,
        date_to: parse_date(q.date_to.as_deref(), true)?,
        ascending: q.order.as_deref() == Some("asc"),
    };
    let (rows, total) = app.store.list_payouts(auth.project_id, &filter, Page::new(limit, page)).await?;
    let catalog = Catalog::load(&app).await?;
    Ok(Json(json!({
        "payouts": rows.iter().map(|p| payout_json(p, &catalog)).collect::<Vec<_>>(),
        "limit": limit,
        "page": page,
        "pagesCount": (total + limit - 1) / limit,
        "total": total,
    })))
}

#[derive(Deserialize)]
struct FeeQuery {
    currency: String,
    amount: Option<String>,
}

async fn fee(State(app): State<App>, key: ApiKey, Params(q): Params<FeeQuery>) -> Result<Json<Value>> {
    let amount = match q.amount.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(a) => Some(a.parse::<Decimal>().map_err(|_| Error::BadRequest("`amount` is not a number.".into()))?),
        None => None,
    };
    let an = app.store.asset_network_by_ticker(&q.currency).await?;
    let (native_fee, native_ticker, fee) = service::estimate_fee(&app, key.project_id, &an.ticker).await?;
    Ok(Json(json!({
        "currency": an.ticker,
        "amount": amount.map(|a| a.normalize().to_string()),
        "fee": num(fee),
        "network_fee": num(native_fee),
        "fee_currency": native_ticker,
    })))
}
