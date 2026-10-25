//! Payout actions shared by the API and the dashboard: create, verify, approve, reject, cancel, resolve.

use chrono::{Duration, Utc};
use rust_decimal::Decimal;
use sqlx::PgConnection;

use crate::{
    app::App,
    auth::totp,
    db::{NewPayout, PayoutRow, PgStore, UserRow},
    domain::{AssetNetwork, Network},
    error::{Error, Result},
    payouts::{
        PayoutStatus, approval, chain, check_family,
        limits::{Decision, PayoutLimits},
        notify_logged,
        rpc::Nodes,
        to_base_units,
    },
};

pub const MAX_BATCH: usize = 100;
pub const MAX_MEMO: usize = 120;

/// One withdrawal of a request.
#[derive(Clone, Debug, Default)]
pub struct NewWithdrawal {
    pub address: String,
    pub currency: String,
    pub amount: Decimal,
    pub extra_id: Option<String>,
    pub ipn_callback_url: Option<String>,
    pub description: Option<String>,
    pub unique_external_id: Option<String>,
}

/// Who creates a batch.
#[derive(Clone, Copy, Debug)]
pub enum Source {
    /// Merchant API: only address-book destinations; waits for `verify`.
    Api { user_id: i64, api_key_id: i64 },
    /// Dashboard: any valid destination; the form took a TOTP code, so the limits apply at once.
    Dashboard { user_id: i64 },
}

impl Source {
    fn user_id(self) -> i64 {
        match self {
            Source::Api { user_id, .. } | Source::Dashboard { user_id } => user_id,
        }
    }
}

pub struct BatchOutcome {
    pub batch_id: i64,
    /// In request order. Withdrawals that already existed (same `unique_external_id`) keep their own batch.
    pub payouts: Vec<PayoutRow>,
}

fn clean(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// The canonical form of an address on a network (EIP-55 on EVM). Refuses invalid addresses.
pub async fn normalize_address(app: &App, network: &Network, address: &str) -> Result<String> {
    let address = address.trim();
    #[cfg(feature = "evm")]
    if network.family == "evm" {
        return crate::payouts::evm::normalize_address(address);
    }
    if !crate::addresses::validate(app, network, address).await? {
        return Err(Error::BadRequest(format!("`{address}` is not a valid {} address.", network.name)));
    }
    #[cfg(feature = "tron")]
    if network.family == "tron" {
        crate::payouts::tron::decode_address(address)?;
    }
    #[cfg(feature = "sol")]
    if network.family == "solana" {
        crate::payouts::sol::decode_pubkey(address)?;
    }
    #[cfg(feature = "ton")]
    if network.family == "ton" {
        crate::payouts::ton::parse_address(address)?;
    }
    Ok(address.to_string())
}

/// USD value of an amount, when a rate is available.
pub async fn usd_value(app: &App, an: &AssetNetwork, amount: Decimal) -> Option<Decimal> {
    let cg = an.asset.coingecko_id.as_deref()?;
    let price = app.rates.price(cg, "usd").await.ok()?;
    Some((amount * price).round_dp(8))
}

/// Same request for an idempotency key?
pub fn same_request(existing: &PayoutRow, asset_network_id: i64, address: &str, extra_id: Option<&str>, amount: Decimal) -> bool {
    existing.asset_network_id == asset_network_id
        && existing.address.eq_ignore_ascii_case(address)
        && existing.extra_id.as_deref() == extra_id
        && existing.amount.normalize() == amount.normalize()
}

/// Check a currency for payouts in a project: a payout network with a hot wallet.
pub async fn payout_asset(app: &App, project_id: i64, currency: &str) -> Result<AssetNetwork> {
    let an = app.store.asset_network_by_ticker(currency).await?;
    check_family(&an.network)?;
    if crate::hot::address_for_project(app, project_id, &an.network.family).await?.is_none() {
        return Err(Error::BadRequest(format!(
            "This project has no {} hot wallet. A server admin assigns one under Server > Hot wallets.",
            an.network.name
        )));
    }
    Ok(an)
}

struct Prepared {
    new: NewPayout,
    existing: Option<PayoutRow>,
}

async fn prepare(
    app: &App,
    project_id: i64,
    source: Source,
    w: NewWithdrawal,
    batch_ipn: &Option<String>,
    limits: &PayoutLimits,
) -> Result<Prepared> {
    let an = payout_asset(app, project_id, &w.currency).await?;
    let network = &an.network;
    let address = normalize_address(app, network, &w.address).await?;
    let mut extra_id = clean(w.extra_id);
    if extra_id.as_ref().is_some_and(|m| m.len() > MAX_MEMO) {
        return Err(Error::BadRequest(format!("`extra_id` is longer than {MAX_MEMO} characters.")));
    }
    if matches!(source, Source::Api { .. }) {
        let book = app.store.payout_addresses_for(project_id, network.network_id, &address).await?;
        let not_in_book = || {
            Error::BadRequest(format!(
                "`{address}` is not in the address book for {}. API payouts only go to address-book entries.",
                network.name
            ))
        };
        match &extra_id {
            Some(m) => {
                if !book.iter().any(|e| e.extra_id.as_deref() == Some(m.as_str())) {
                    return Err(not_in_book());
                }
            }
            None => match book.as_slice() {
                [] => return Err(not_in_book()),
                [one] => extra_id = one.extra_id.clone(),
                _ if book.iter().any(|e| e.extra_id.is_none()) => {}
                _ => return Err(Error::BadRequest("This address has several memos in the address book. Send `extra_id`.".into())),
            },
        }
    }
    to_base_units(w.amount, an.decimals as u32)?;
    let amount = w.amount.normalize();
    let amount_usd = usd_value(app, &an, amount).await;
    limits.check_minimum(amount_usd)?;
    let ipn = clean(w.ipn_callback_url).or_else(|| batch_ipn.clone());
    crate::payments::check_url(app, "ipn_callback_url", &ipn)?;
    let description = clean(w.description);
    if description.as_ref().is_some_and(|d| d.len() > 500) {
        return Err(Error::BadRequest("`payout_description` is longer than 500 characters.".into()));
    }
    let unique_external_id = clean(w.unique_external_id);
    if unique_external_id.as_ref().is_some_and(|d| d.len() > 255) {
        return Err(Error::BadRequest("`unique_external_id` is longer than 255 characters.".into()));
    }
    let existing = match &unique_external_id {
        Some(key) => match app.store.payout_by_external_id(project_id, key).await? {
            Some(p) if same_request(&p, an.asset_network_id, &address, extra_id.as_deref(), amount) => Some(p),
            Some(_) => {
                return Err(Error::Conflict(format!("`unique_external_id` {key} is used by another payout with other fields.")));
            }
            None => None,
        },
        None => None,
    };
    if existing.is_none()
        && let Some(c) = chain::for_family(&network.family)
        && let Ok(nodes) = Nodes::load(app, network).await
    {
        match c.precheck(&nodes, &an, &address, amount).await {
            Err(e @ Error::BadRequest(_)) => return Err(e),
            Err(e) => tracing::debug!(error = %e, "payout precheck skipped"),
            Ok(()) => {}
        }
    }
    Ok(Prepared {
        new: NewPayout {
            asset_network_id: an.asset_network_id,
            network_id: network.network_id,
            address,
            extra_id,
            amount,
            amount_usd,
            status: PayoutStatus::PendingOtpVerification,
            description,
            unique_external_id,
            ipn_callback_url: ipn,
        },
        existing,
    })
}

/// Create a batch. API batches wait for verification; dashboard batches go through the limits at once.
pub async fn create_batch(
    app: &App,
    project_id: i64,
    source: Source,
    withdrawals: Vec<NewWithdrawal>,
    batch_ipn: Option<String>,
) -> Result<BatchOutcome> {
    if withdrawals.is_empty() {
        return Err(Error::BadRequest("`withdrawals` is empty.".into()));
    }
    if withdrawals.len() > MAX_BATCH {
        return Err(Error::BadRequest(format!("A batch can have at most {MAX_BATCH} withdrawals.")));
    }
    let mut keys = std::collections::HashSet::new();
    for w in &withdrawals {
        if let Some(k) = clean(w.unique_external_id.clone())
            && !keys.insert(k.clone())
        {
            return Err(Error::BadRequest(format!("`unique_external_id` {k} is in the request twice.")));
        }
    }
    let batch_ipn = clean(batch_ipn);
    crate::payments::check_url(app, "ipn_callback_url", &batch_ipn)?;
    let limits = app.store.payout_limits(project_id).await?;
    for attempt in 0..2 {
        let mut prepared = Vec::with_capacity(withdrawals.len());
        for w in withdrawals.iter().cloned() {
            prepared.push(prepare(app, project_id, source, w, &batch_ipn, &limits).await?);
        }
        if prepared.iter().all(|p| p.existing.is_some()) {
            let payouts: Vec<PayoutRow> = prepared.into_iter().filter_map(|p| p.existing).collect();
            return Ok(BatchOutcome { batch_id: payouts[0].batch_id, payouts });
        }
        match insert_batch(app, project_id, source, &prepared, &limits).await {
            Ok(out) => return Ok(out),
            // Another request took a `unique_external_id` meanwhile: read it again.
            Err(Error::Conflict(_)) if attempt == 0 => continue,
            Err(e) => return Err(e),
        }
    }
    Err(Error::Conflict("The request conflicts with a payout made at the same time. Send it again.".into()))
}

async fn insert_batch(app: &App, project_id: i64, source: Source, prepared: &[Prepared], limits: &PayoutLimits) -> Result<BatchOutcome> {
    let mut tx = app.store.pool.begin().await?;
    let (kind, api_key_id) = match source {
        Source::Api { api_key_id, .. } => ("api", Some(api_key_id)),
        Source::Dashboard { .. } => ("dashboard", None),
    };
    let user_id = source.user_id();
    let batch_id = PgStore::insert_payout_batch(&mut tx, project_id, kind, Some(user_id), api_key_id).await?;
    let mut out = Vec::with_capacity(prepared.len());
    let mut fresh = Vec::new();
    for p in prepared {
        if let Some(e) = &p.existing {
            out.push(e.clone());
            continue;
        }
        match PgStore::insert_payout(&mut tx, project_id, batch_id, &p.new, Some(user_id)).await {
            Ok(row) => {
                fresh.push(out.len());
                out.push(row);
            }
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                return Err(Error::Conflict("`unique_external_id` is taken.".into()));
            }
            Err(e) => return Err(e.into()),
        }
    }
    if matches!(source, Source::Dashboard { .. }) {
        let rows: Vec<PayoutRow> = fresh.iter().map(|i| out[*i].clone()).collect();
        let cleared = clear(&mut tx, project_id, limits, &rows, Some(user_id)).await?;
        for (i, row) in fresh.iter().zip(cleared) {
            out[*i] = row;
        }
    }
    tx.commit().await?;
    // An API payout sends its first webhook when it is verified. A dashboard payout is verified now.
    if matches!(source, Source::Dashboard { .. }) {
        for i in &fresh {
            notify_logged(app, &out[*i]).await;
        }
    }
    Ok(BatchOutcome { batch_id, payouts: out })
}

/// Apply the limits to verified payouts, under a per-project lock so that parallel requests count each other.
async fn clear(
    conn: &mut PgConnection,
    project_id: i64,
    limits: &PayoutLimits,
    payouts: &[PayoutRow],
    verified_by: Option<i64>,
) -> Result<Vec<PayoutRow>> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('payout-limits:' || $1::text))").bind(project_id).execute(&mut *conn).await?;
    let mut out = Vec::with_capacity(payouts.len());
    for p in payouts {
        let now = Utc::now();
        let used_hour = PgStore::payout_usage_usd(conn, project_id, now - Duration::hours(1)).await?;
        let used_day = PgStore::payout_usage_usd(conn, project_id, now - Duration::hours(24)).await?;
        let (to, reason) = match limits.decide(p.amount_usd, used_hour, used_day) {
            Decision::Auto => (PayoutStatus::Pending, None),
            Decision::NeedsApproval(r) => (PayoutStatus::PendingApproval, Some(r.as_str())),
        };
        let row = PgStore::set_payout_cleared(conn, p.payout_id, &[PayoutStatus::PendingOtpVerification], to, reason, verified_by)
            .await?
            .unwrap_or_else(|| p.clone());
        out.push(row);
    }
    Ok(out)
}

/// `POST /v1/payout/{batch}/verify` after the TOTP check: unverified payouts of the batch go through the limits.
pub async fn verify_batch(app: &App, project_id: i64, batch_id: i64, user_id: i64) -> Result<Vec<PayoutRow>> {
    if !app.store.batch_exists(project_id, batch_id).await? {
        return Err(Error::NotFound);
    }
    let limits = app.store.payout_limits(project_id).await?;
    let mut tx = app.store.pool.begin().await?;
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM payouts WHERE project_id = $1 AND batch_id = $2 AND status = 'pending-otp-verification' \
         AND verified_at IS NULL ORDER BY payout_id FOR UPDATE",
        crate::db::PAYOUT_COLS
    )))
    .bind(project_id)
    .bind(batch_id)
    .fetch_all(&mut *tx)
    .await?;
    let waiting: Vec<PayoutRow> = rows.iter().map(crate::db::payout_from_row).collect::<Result<_>>()?;
    let changed = clear(&mut tx, project_id, &limits, &waiting, Some(user_id)).await?;
    tx.commit().await?;
    for p in &changed {
        notify_logged(app, p).await;
    }
    app.store.batch_payouts(project_id, batch_id).await
}

/// The TOTP check for a user who is not in a dashboard session (API bearer token). Same rules as the dashboard:
/// a code works once, 5 wrong codes lock for 15 minutes, and on mainnet two-factor must be on.
pub async fn check_user_totp(app: &App, user: &UserRow, code: &str) -> Result<()> {
    if !user.totp_enabled {
        if app.config.mode.is_mainnet() {
            return Err(Error::Forbidden("Turn on two-factor authentication on your account page first.".into()));
        }
        return Ok(());
    }
    let key = format!("totp:{}", user.user_id);
    if app.limits.totp.is_blocked(&key) {
        return Err(Error::TooManyRequests);
    }
    let secret = match &user.totp_secret {
        Some(s) => app.sealer.open("totp", s)?,
        None => return Err(Error::Internal("two-factor secret missing".into())),
    };
    let now = Utc::now().timestamp().max(0) as u64;
    match totp::verify(&secret, code, now, user.totp_last_step.max(0) as u64) {
        Some(step) if app.store.consume_totp_step(user.user_id, step as i64).await? => {
            app.limits.totp.reset(&key);
            Ok(())
        }
        _ => {
            app.limits.totp.hit(&key);
            Err(Error::BadRequest("The verification code is wrong or was already used.".into()))
        }
    }
}

async fn project_payout(app: &App, project_id: i64, payout_id: i64) -> Result<PayoutRow> {
    app.store.payout(project_id, payout_id).await?.ok_or(Error::NotFound)
}

/// Approve, step 1 (the caller checked the admin's TOTP code): email that admin a code. Returns the code
/// so that tests can enter it; the dashboard never shows it.
pub async fn start_approval(app: &App, project_id: i64, payout_id: i64, user: &UserRow) -> Result<String> {
    project_payout(app, project_id, payout_id).await?;
    let mut tx = app.store.pool.begin().await?;
    let p = PgStore::payout_for_update(&mut tx, payout_id).await?.ok_or(Error::NotFound)?;
    let waits = match p.status {
        PayoutStatus::PendingApproval => true,
        PayoutStatus::PendingOtpVerification if p.verified_at.is_some() => false,
        PayoutStatus::PendingOtpVerification => {
            return Err(Error::Conflict("This payout waits for verification through the API.".into()));
        }
        _ => return Err(Error::Conflict("This payout does not wait for an approval.".into())),
    };
    let code = approval::new_code();
    let now = Utc::now();
    PgStore::put_approval_code(
        &mut tx,
        payout_id,
        user.user_id,
        &approval::code_hash(app, payout_id, user.user_id, &code),
        approval::expiry(now),
    )
    .await?;
    let changed = if waits {
        PgStore::set_payout_status(&mut tx, payout_id, &[PayoutStatus::PendingApproval], PayoutStatus::PendingOtpVerification, None).await?
    } else {
        None
    };
    let an = app.store.asset_network(p.asset_network_id).await?;
    let project = app.store.project(project_id).await?;
    let body = format!(
        "Your code to approve payout {payout_id} of {}:\n\n{code}\n\nAmount: {} {} on {}\nTo: {}{}\n\n\
         The code works for {} minutes, for you only. If you did not click Approve, do not use it and tell \
         the other admins.\n",
        project.name,
        p.amount.normalize(),
        an.asset.symbol,
        an.network.name,
        p.address,
        p.extra_id.as_deref().map(|m| format!(" (memo {m})")).unwrap_or_default(),
        approval::CODE_TTL_MINUTES,
    );
    app.mailer.send(&user.email, &format!("Payout approval code: {code}"), &body).await?;
    tx.commit().await?;
    if let Some(p) = changed {
        notify_logged(app, &p).await;
    }
    Ok(code)
}

/// Approve, step 2: the emailed code. Only the admin who got it can enter it.
pub async fn confirm_approval(app: &App, project_id: i64, payout_id: i64, user_id: i64, code: &str) -> Result<PayoutRow> {
    project_payout(app, project_id, payout_id).await?;
    let mut tx = app.store.pool.begin().await?;
    let p = PgStore::payout_for_update(&mut tx, payout_id).await?.ok_or(Error::NotFound)?;
    if p.status != PayoutStatus::PendingOtpVerification || p.verified_at.is_none() {
        return Err(Error::Conflict("This payout does not wait for an emailed code.".into()));
    }
    let row = PgStore::approval_code_for_update(&mut tx, payout_id)
        .await?
        .ok_or_else(|| Error::Conflict("No code is active. Click Approve to get a new code.".into()))?;
    let verdict = approval::check(&row, user_id, code, Utc::now(), |u, c| approval::code_hash(app, payout_id, u, c));
    match verdict {
        approval::Verdict::Ok => {
            PgStore::delete_approval_code(&mut tx, payout_id).await?;
            let p = PgStore::set_payout_approved(&mut tx, payout_id, user_id)
                .await?
                .ok_or_else(|| Error::Conflict("The payout changed. Reload the page.".into()))?;
            tx.commit().await?;
            notify_logged(app, &p).await;
            Ok(p)
        }
        approval::Verdict::OtherAdmin => Err(Error::Forbidden("The code was sent to another admin. Only that admin can enter it.".into())),
        approval::Verdict::Wrong { left } if left > 0 => {
            PgStore::bump_approval_attempts(&mut tx, payout_id).await?;
            tx.commit().await?;
            Err(Error::BadRequest(format!("Wrong code. {left} attempt{} left.", if left == 1 { "" } else { "s" })))
        }
        approval::Verdict::Wrong { .. } => {
            let changed = back_to_approval(&mut tx, payout_id).await?;
            tx.commit().await?;
            if let Some(p) = changed {
                notify_logged(app, &p).await;
            }
            Err(Error::BadRequest("Wrong code, and no attempts are left. The payout waits for approval again.".into()))
        }
        approval::Verdict::Expired => {
            let changed = back_to_approval(&mut tx, payout_id).await?;
            tx.commit().await?;
            if let Some(p) = changed {
                notify_logged(app, &p).await;
            }
            Err(Error::BadRequest("The code expired. Click Approve to get a new one.".into()))
        }
    }
}

/// The emailed code is used up: delete it and wait for a new approval.
async fn back_to_approval(conn: &mut PgConnection, payout_id: i64) -> Result<Option<PayoutRow>> {
    PgStore::delete_approval_code(conn, payout_id).await?;
    PgStore::set_payout_status(conn, payout_id, &[PayoutStatus::PendingOtpVerification], PayoutStatus::PendingApproval, None).await
}

/// End a payout that waits for a person.
pub async fn reject(app: &App, project_id: i64, payout_id: i64) -> Result<PayoutRow> {
    end(
        app,
        project_id,
        payout_id,
        &PayoutStatus::REJECTABLE,
        PayoutStatus::Rejected,
        "Only a payout that waits for approval can be rejected.",
    )
    .await
}

/// Stop a payout before it is signed.
pub async fn cancel(app: &App, project_id: i64, payout_id: i64) -> Result<PayoutRow> {
    end(
        app,
        project_id,
        payout_id,
        &PayoutStatus::CANCELLABLE,
        PayoutStatus::Cancelled,
        "The payout is already signed. It cannot be cancelled.",
    )
    .await
}

async fn end(app: &App, project_id: i64, payout_id: i64, from: &[PayoutStatus], to: PayoutStatus, refusal: &str) -> Result<PayoutRow> {
    project_payout(app, project_id, payout_id).await?;
    let mut tx = app.store.pool.begin().await?;
    // Waits while the signer holds the row; then the status is `signing` and nothing changes.
    let p = PgStore::set_payout_status(&mut tx, payout_id, from, to, None).await?.ok_or_else(|| Error::Conflict(refusal.into()))?;
    PgStore::delete_approval_code(&mut tx, payout_id).await?;
    tx.commit().await?;
    notify_logged(app, &p).await;
    Ok(p)
}

/// Decide a `needs-review` payout after checking the chain by hand.
pub async fn resolve(app: &App, project_id: i64, payout_id: i64, sent: bool, note: &str) -> Result<PayoutRow> {
    let note = note.trim();
    if note.is_empty() || note.len() > 1000 {
        return Err(Error::BadRequest("Write a note of 1 to 1000 characters: what you checked.".into()));
    }
    let p = app
        .store
        .resolve_payout(project_id, payout_id, sent, note)
        .await?
        .ok_or_else(|| Error::Conflict("Only a payout that needs review can be resolved.".into()))?;
    notify_logged(app, &p).await;
    Ok(p)
}

/// Add an address-book entry. Returns its id and the canonical address.
pub async fn add_address(
    app: &App,
    project_id: i64,
    network_id: i64,
    label: &str,
    address: &str,
    extra_id: Option<&str>,
    user_id: i64,
) -> Result<(i64, String)> {
    let network = app.store.network(network_id).await?;
    check_family(&network)?;
    let label = label.trim();
    if label.is_empty() || label.chars().count() > 80 {
        return Err(Error::BadRequest("Give a label of 1 to 80 characters.".into()));
    }
    let address = normalize_address(app, &network, address).await?;
    let memo = extra_id.map(str::trim).filter(|m| !m.is_empty());
    if memo.is_some_and(|m| m.len() > MAX_MEMO) {
        return Err(Error::BadRequest(format!("The memo is longer than {MAX_MEMO} characters.")));
    }
    let id = app.store.add_payout_address(project_id, network_id, label, &address, memo, Some(user_id)).await?;
    Ok((id, address))
}

/// Network fee estimate: (fee in the native coin, the native ticker, fee in `currency`).
pub async fn estimate_fee(app: &App, project_id: i64, currency: &str) -> Result<(Decimal, String, Decimal)> {
    let an = app.store.asset_network_by_ticker(currency).await?;
    check_family(&an.network)?;
    let c =
        chain::for_family(&an.network.family).ok_or_else(|| Error::Unavailable("This build has no payouts for this network.".into()))?;
    let nodes = Nodes::load(app, &an.network).await?;
    let from = crate::hot::address_for_project(app, project_id, &an.network.family).await?.map(|(_, a)| a);
    let fee = c.estimate_fee(&nodes, &an, from.as_deref()).await?;
    let all = app.store.list_asset_networks().await?;
    let native = all.iter().find(|a| a.network.network_id == an.network.network_id && a.is_native()).ok_or(Error::NotFound)?.clone();
    if an.is_native() {
        return Ok((fee, native.ticker, fee));
    }
    let price = |a: &AssetNetwork| {
        let cg = a.asset.coingecko_id.clone();
        async move {
            let cg = cg.ok_or_else(|| Error::Rate("no price source".into()))?;
            app.rates.price(&cg, "usd").await
        }
    };
    let (native_usd, token_usd) = (price(&native).await?, price(&an).await?);
    if token_usd <= Decimal::ZERO {
        return Err(Error::Rate("no token price".into()));
    }
    Ok((fee, native.ticker, (fee * native_usd / token_usd).round_dp(an.decimals as u32)))
}

/// Native balance of an address on a network, through its nodes.
pub async fn native_balance(app: &App, network: &Network, address: &str) -> Result<Decimal> {
    let c = chain::for_family(&network.family).ok_or_else(|| Error::Unavailable("no payouts on this network".into()))?;
    let nodes = Nodes::load(app, network).await?;
    c.native_balance(&nodes, address).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(amount: &str) -> PayoutRow {
        PayoutRow {
            payout_id: 1,
            batch_id: 1,
            project_id: 1,
            asset_network_id: 5,
            network_id: 2,
            address: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266".into(),
            extra_id: None,
            amount: amount.parse().unwrap(),
            amount_usd: None,
            status: PayoutStatus::Pending,
            status_seq: 0,
            approval_reason: None,
            description: None,
            unique_external_id: Some("refund-1".into()),
            ipn_callback_url: None,
            hot_wallet_id: None,
            from_address: None,
            txid: None,
            fee: None,
            confirmations: 0,
            error: None,
            resolution_note: None,
            created_by: None,
            verified_by: None,
            verified_at: None,
            approved_by: None,
            approved_at: None,
            requested_at: None,
            refused_at: None,
            next_check_at: Utc::now(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn idempotency_compares_the_fields() {
        let p = row("250.00");
        let addr = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266";
        assert!(same_request(&p, 5, addr, None, "250".parse().unwrap()));
        assert!(!same_request(&p, 5, addr, None, "250.01".parse().unwrap()));
        assert!(!same_request(&p, 6, addr, None, "250".parse().unwrap()));
        assert!(!same_request(&p, 5, addr, Some("memo"), "250".parse().unwrap()));
        assert!(!same_request(&p, 5, "0x70997970C51812dc3A010C7d01b50e0d17dc79C8", None, "250".parse().unwrap()));
    }
}
