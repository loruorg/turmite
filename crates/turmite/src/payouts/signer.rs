//! The signer loop (worker). For each `pending` payout: lock it, sign, store the signed transaction, then
//! broadcast it. Then track it until it has the network's confirmations.
//!
//! - A refused broadcast does not fail the payout at once. After 2 minutes the payout fails only when no node
//!   knows the transaction.
//! - EVM: a transaction that waits in the mempool is signed again with the same nonce and higher fees.
//! - Solana, Tron, TON: an expired transaction that no node knows is signed again.
//! - When the nonce is used but by no transaction we know, the payout is `needs-review` after 30 minutes.

use std::time::Duration;

use chrono::Utc;
use sqlx::{Connection, PgConnection};

use crate::{
    app::App,
    db::{AttemptRow, PayoutRow, PgStore},
    domain::AssetNetwork,
    error::{Error, Result},
    hot::{self, HotKey},
    payouts::{
        PayoutStatus,
        chain::{self, Broadcast, Landing, PayoutChain, SignCtx, SignMode, TxState},
        notify_logged,
        rpc::Nodes,
    },
};

const TICK_SECS: u64 = 5;
/// A refused transaction fails when no node knows it this long after the first refusal.
const REFUSAL_GRACE_SECS: i64 = 120;
/// A used nonce with no matching transaction: `needs-review` after this long.
const REVIEW_AFTER_SECS: i64 = 30 * 60;
const BATCH: i64 = 20;

/// Start the loop. Called once from `worker::spawn`.
pub fn spawn(app: App) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(TICK_SECS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            tick(&app).await;
        }
    });
}

/// One pass: expire approval codes, sign pending payouts, advance the ones on their way.
pub async fn tick(app: &App) {
    match app.store.expire_approval_codes().await {
        Ok(rows) => {
            for p in rows {
                notify_logged(app, &p).await;
            }
        }
        Err(e) => tracing::warn!(error = %e, "expiring payout approval codes failed"),
    }
    if app.hot_identity().is_some() {
        match app.store.due_payout_ids(&[PayoutStatus::Pending], BATCH).await {
            Ok(ids) => {
                for id in ids {
                    if let Err(e) = sign_new(app, id).await {
                        tracing::warn!(payout = id, error = %e, "payout signing failed");
                    }
                }
            }
            Err(e) => tracing::warn!(error = %e, "listing pending payouts failed"),
        }
    }
    match app.store.due_payout_ids(&[PayoutStatus::Signing, PayoutStatus::Initiated], BATCH * 5).await {
        Ok(ids) => {
            for id in ids {
                if let Err(e) = advance(app, id).await {
                    tracing::warn!(payout = id, error = %e, "payout tracking failed");
                    let _ = app.store.defer_payout(id, None, 30).await;
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "listing payouts on their way failed"),
    }
}

/// Run until every payout of `ids` is final or `needs-review`, or `timeout` passes. For tests.
pub async fn run_until_settled(app: &App, ids: &[i64], timeout: Duration) -> Result<Vec<PayoutRow>> {
    let start = std::time::Instant::now();
    loop {
        sqlx::query("UPDATE payouts SET next_check_at = now() WHERE payout_id = ANY($1)").bind(ids).execute(&app.store.pool).await?;
        tick(app).await;
        let mut rows = Vec::new();
        for id in ids {
            rows.push(app.store.payout_any(*id).await?.ok_or(Error::NotFound)?);
        }
        if rows.iter().all(|p| p.status.is_final() || p.status == PayoutStatus::NeedsReview) || start.elapsed() > timeout {
            return Ok(rows);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

struct Ctx {
    an: AssetNetwork,
    chain: &'static dyn PayoutChain,
    nodes: Nodes,
}

async fn context(app: &App, p: &PayoutRow) -> Result<std::result::Result<Ctx, String>> {
    let an = app.store.asset_network(p.asset_network_id).await?;
    let Some(chain) = chain::for_family(&an.network.family) else {
        return Ok(Err(format!("This build cannot send payouts on {}.", an.network.name)));
    };
    match Nodes::load(app, &an.network).await {
        Ok(nodes) => Ok(Ok(Ctx { an, chain, nodes })),
        Err(e) => Ok(Err(e.public_message())),
    }
}

/// A connection outside the pool: it holds the wallet's session lock and closes when dropped, so the lock
/// can never leak into the pool.
async fn locked_conn(app: &App, c: &Ctx, address: &str) -> Result<(PgConnection, Option<String>)> {
    let mut conn = app.store.pool.acquire().await?.detach();
    let key = c.chain.lock_key(&c.nodes.network.chain, address);
    if let Some(k) = &key {
        sqlx::query("SELECT pg_advisory_lock(hashtext($1))").bind(k).execute(&mut conn).await?;
    }
    Ok((conn, key))
}

async fn unlock(mut conn: PgConnection, key: Option<String>) {
    if let Some(k) = key {
        let _ = sqlx::query("SELECT pg_advisory_unlock(hashtext($1))").bind(k).execute(&mut conn).await;
    }
    let _ = conn.close().await;
}

async fn fail(app: &App, p: &PayoutRow, from: &[PayoutStatus], error: &str) -> Result<()> {
    if let Some(row) = app.store.finish_payout(p.payout_id, from, PayoutStatus::Failed, None, None, 0, Some(error)).await? {
        notify_logged(app, &row).await;
    }
    Ok(())
}

/// Sign and broadcast a `pending` payout.
async fn sign_new(app: &App, payout_id: i64) -> Result<()> {
    let Some(p) = app.store.payout_any(payout_id).await? else { return Ok(()) };
    if p.status != PayoutStatus::Pending {
        return Ok(());
    }
    let c = match context(app, &p).await? {
        Ok(c) => c,
        Err(msg) => return app.store.defer_payout(payout_id, Some(&msg), 60).await,
    };
    let Some(key) = hot::key_for_project(app, p.project_id, &c.an.network.family).await? else {
        return fail(app, &p, &[PayoutStatus::Pending], &format!("The project has no {} hot wallet.", c.an.network.name)).await;
    };
    // An earlier payout of this wallet that no node took yet: wait, so that nonces stay in order.
    if waits_for_earlier(app, &p, &key).await? {
        return app.store.defer_payout(payout_id, None, 15).await;
    }
    let (mut conn, lock) = locked_conn(app, &c, &key.address).await?;
    let signed = sign_and_store(app, &c, &mut conn, payout_id, &key, SignMode::New).await;
    let result = match signed {
        Ok(Some((row, attempt))) => {
            notify_logged(app, &row).await;
            push(app, &c, &row, &attempt).await
        }
        Ok(None) => Ok(()),
        Err(e) => Err(e),
    };
    unlock(conn, lock).await;
    result
}

async fn waits_for_earlier(app: &App, p: &PayoutRow, key: &HotKey) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM payouts WHERE network_id = $1 AND LOWER(from_address) = LOWER($2) \
           AND status = 'signing' AND payout_id <> $3)",
    )
    .bind(p.network_id)
    .bind(&key.address)
    .bind(p.payout_id)
    .fetch_one(&app.store.pool)
    .await?)
}

/// Lock the payout, sign, store the attempt and commit. Returns the payout and the stored attempt, or None
/// when the payout changed or failed.
async fn sign_and_store(
    app: &App,
    c: &Ctx,
    conn: &mut PgConnection,
    payout_id: i64,
    key: &HotKey,
    mode: SignMode<'_>,
) -> Result<Option<(PayoutRow, AttemptRow)>> {
    let first = matches!(mode, SignMode::New);
    let mut tx = conn.begin().await?;
    let locked =
        if first { PgStore::claim_pending_payout(&mut tx, payout_id).await? } else { PgStore::lock_payout(&mut tx, payout_id).await? };
    let Some(p) = locked else { return Ok(None) };
    if !first && !matches!(p.status, PayoutStatus::Signing | PayoutStatus::Initiated) {
        return Ok(None);
    }
    let ctx = SignCtx { app, nodes: &c.nodes, an: &c.an, payout: &p, key };
    match c.chain.sign(&ctx, &mut tx, mode).await {
        Ok(attempt) => {
            let attempt_id = PgStore::insert_payout_attempt(&mut tx, payout_id, p.network_id, &attempt).await?;
            let row =
                PgStore::set_payout_signed(&mut tx, payout_id, key.hot_wallet_id, &attempt.from_address, &attempt.txid, first).await?;
            tx.commit().await?;
            let stored = app
                .store
                .payout_attempts(payout_id)
                .await?
                .into_iter()
                .find(|a| a.payout_attempt_id == attempt_id)
                .ok_or(Error::NotFound)?;
            Ok(Some((row, stored)))
        }
        Err(Error::BadRequest(msg)) if first => {
            let failed = PgStore::set_payout_status(&mut tx, payout_id, &[PayoutStatus::Pending], PayoutStatus::Failed, Some(&msg)).await?;
            tx.commit().await?;
            if let Some(row) = failed {
                notify_logged(app, &row).await;
            }
            Ok(None)
        }
        Err(e) => {
            drop(tx);
            app.store.defer_payout(payout_id, Some(&e.public_message_for_payout()), 60).await?;
            Ok(None)
        }
    }
}

/// Broadcast a stored attempt and record what the nodes said.
async fn push(app: &App, c: &Ctx, p: &PayoutRow, a: &AttemptRow) -> Result<()> {
    match c.chain.broadcast(&c.nodes, a).await {
        Broadcast::Accepted => {
            app.store.mark_attempt_broadcast(a.payout_attempt_id).await?;
            if p.status == PayoutStatus::Signing
                && let Some(row) = app.store.set_payout_initiated(p.payout_id, &a.txid).await?
            {
                notify_logged(app, &row).await;
            }
            app.store.defer_payout(p.payout_id, None, TICK_SECS as i64).await
        }
        Broadcast::Unreachable(msg) => {
            app.store.mark_attempt_error(a.payout_attempt_id, &msg).await?;
            app.store.defer_payout(p.payout_id, Some("No node answered. Turmite tries again."), 30).await
        }
        Broadcast::Refused(msg) => {
            app.store.mark_attempt_error(a.payout_attempt_id, &msg).await?;
            refused(app, c, p, a, &msg).await
        }
    }
}

async fn refused(app: &App, c: &Ctx, p: &PayoutRow, a: &AttemptRow, msg: &str) -> Result<()> {
    // A refused fee bump changes nothing: the earlier transaction with the same nonce stays valid.
    let attempts = app.store.payout_attempts(p.payout_id).await?;
    if a.nonce.is_some()
        && attempts.iter().any(|o| o.payout_attempt_id != a.payout_attempt_id && o.nonce == a.nonce && o.broadcast_at.is_some())
        && c.chain.bump_after_secs().is_some()
    {
        return app.store.defer_payout(p.payout_id, None, 30).await;
    }
    let first = p.refused_at.unwrap_or_else(Utc::now);
    if (Utc::now() - first).num_seconds() < REFUSAL_GRACE_SECS {
        return app.store.set_payout_refused(p.payout_id, &format!("The node refused the transaction: {msg}"), 20).await;
    }
    match c.chain.state(&c.nodes, a).await? {
        TxState::Unknown => {
            fail(app, p, &[PayoutStatus::Signing, PayoutStatus::Initiated], &format!("Every node refused the transaction: {msg}")).await
        }
        // A node has it after all: track it.
        _ => {
            if let Some(row) = app.store.set_payout_initiated(p.payout_id, &a.txid).await? {
                notify_logged(app, &row).await;
            }
            app.store.defer_payout(p.payout_id, None, TICK_SECS as i64).await
        }
    }
}

/// Track a `signing` or `initiated` payout.
async fn advance(app: &App, payout_id: i64) -> Result<()> {
    let Some(p) = app.store.payout_any(payout_id).await? else { return Ok(()) };
    if !matches!(p.status, PayoutStatus::Signing | PayoutStatus::Initiated) {
        return Ok(());
    }
    let c = match context(app, &p).await? {
        Ok(c) => c,
        Err(msg) => return app.store.defer_payout(payout_id, Some(&msg), 60).await,
    };
    let attempts = app.store.payout_attempts(payout_id).await?;
    let Some(latest) = attempts.last().cloned() else {
        return fail(app, &p, &[PayoutStatus::Signing, PayoutStatus::Initiated], "No signed transaction was stored.").await;
    };
    let required = c.an.network.confirmations_required.max(1) as i64;
    let mut pending: Option<&AttemptRow> = None;
    let mut unreachable = false;
    for a in attempts.iter().rev() {
        match c.chain.state(&c.nodes, a).await {
            Ok(TxState::Included { confirmations, success, fee, error }) => {
                if !success {
                    let row = app
                        .store
                        .finish_payout(
                            payout_id,
                            &[PayoutStatus::Signing, PayoutStatus::Initiated],
                            PayoutStatus::Failed,
                            Some(&a.txid),
                            fee,
                            confirmations as i32,
                            error.as_deref(),
                        )
                        .await?;
                    if let Some(row) = row {
                        notify_logged(app, &row).await;
                    }
                    return Ok(());
                }
                if confirmations >= required {
                    let row = app
                        .store
                        .finish_payout(
                            payout_id,
                            &[PayoutStatus::Signing, PayoutStatus::Initiated],
                            PayoutStatus::Sent,
                            Some(&a.txid),
                            fee,
                            confirmations.min(i32::MAX as i64) as i32,
                            None,
                        )
                        .await?;
                    if let Some(row) = row {
                        notify_logged(app, &row).await;
                    }
                    return Ok(());
                }
                if p.status == PayoutStatus::Signing
                    && let Some(row) = app.store.set_payout_initiated(payout_id, &a.txid).await?
                {
                    notify_logged(app, &row).await;
                }
                return app.store.set_payout_progress(payout_id, &a.txid, confirmations as i32, TICK_SECS as i64).await;
            }
            Ok(TxState::Pending) => {
                if pending.is_none() {
                    pending = Some(a);
                }
            }
            Ok(TxState::Unknown) => {}
            Err(e) => {
                tracing::debug!(payout = payout_id, error = %e, "payout state check failed");
                unreachable = true;
            }
        }
    }
    if let Some(a) = pending {
        if p.status == PayoutStatus::Signing
            && let Some(row) = app.store.set_payout_initiated(payout_id, &a.txid).await?
        {
            notify_logged(app, &row).await;
        }
        if let Some(after) = c.chain.bump_after_secs()
            && (Utc::now() - latest.created_at).num_seconds() > after
            && app.hot_identity().is_some()
        {
            return resign(app, &c, &p, SignMode::Bump(&latest)).await;
        }
        return app.store.defer_payout(payout_id, None, 10).await;
    }
    if unreachable {
        return app.store.defer_payout(payout_id, Some("No node answered. Turmite tries again."), 30).await;
    }
    // No node knows any of our transactions.
    if p.status == PayoutStatus::Signing || latest.broadcast_at.is_none() {
        return push(app, &c, &p, &latest).await;
    }
    match c.chain.landing(&c.nodes, &latest).await? {
        Landing::CanLand => push(app, &c, &p, &latest).await,
        Landing::Expired if app.hot_identity().is_some() => resign(app, &c, &p, SignMode::Renew).await,
        Landing::Expired => app.store.defer_payout(payout_id, None, 60).await,
        Landing::NonceUsed => {
            let since = latest.broadcast_at.unwrap_or(latest.created_at);
            if (Utc::now() - since).num_seconds() < REVIEW_AFTER_SECS {
                return app.store.defer_payout(payout_id, None, 60).await;
            }
            let row = PgStore::set_payout_status(
                &mut *app.store.pool.acquire().await?,
                payout_id,
                &[PayoutStatus::Initiated],
                PayoutStatus::NeedsReview,
                Some("The nonce of this payout is used, but not by a transaction Turmite signed for it. Check the hot wallet on an explorer, then resolve the payout."),
            )
            .await?;
            if let Some(row) = row {
                notify_logged(app, &row).await;
            }
            Ok(())
        }
    }
}

/// Sign again (fee bump or renewal) with the key that signed the payout, then broadcast.
async fn resign(app: &App, c: &Ctx, p: &PayoutRow, mode: SignMode<'_>) -> Result<()> {
    let Some(hot_wallet_id) = p.hot_wallet_id else { return Ok(()) };
    let key = hot::key_by_id(app, hot_wallet_id).await?;
    let (mut conn, lock) = locked_conn(app, c, &key.address).await?;
    let result = match sign_and_store(app, c, &mut conn, p.payout_id, &key, mode).await {
        Ok(Some((row, attempt))) => push(app, c, &row, &attempt).await,
        Ok(None) => Ok(()),
        Err(e) => Err(e),
    };
    unlock(conn, lock).await;
    result
}

trait PayoutMessage {
    fn public_message_for_payout(&self) -> String;
}

impl PayoutMessage for Error {
    /// Node and chain errors are useful to the operator; internal ones are not shown.
    fn public_message_for_payout(&self) -> String {
        match self {
            Error::Driver(m) | Error::Unavailable(m) => m.clone(),
            other => other.public_message(),
        }
    }
}
