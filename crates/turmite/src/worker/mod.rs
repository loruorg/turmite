//! Background work: chain watching, confirmations, expiry, sweeps, webhook delivery.

#[cfg(feature = "evm")]
pub mod evm_sweeper;
pub mod pool;

use std::time::Duration;

use chrono::Utc;
use rust_decimal::Decimal;

use crate::{
    app::App,
    chains::{
        AssetContext,
        driver::{SweepCheck, SweepRequest, SweepState},
    },
    db::PaymentRow,
    domain::PaymentStatus,
    error::{Error, Result},
    events::DomainEvent,
    reducer, webhooks,
};

/// A sweep that failed on chain is sent again at once this many times; after that, once per
/// `SLOW_SWEEP_RETRY_MINUTES`, because each failed try can cost network fees.
pub const QUICK_SWEEP_RETRIES: i64 = 3;
const SLOW_SWEEP_RETRY_MINUTES: i64 = 60;
/// How often the worker checks the sweeps of payments it no longer watches.
const SWEEP_BACKLOG_SECONDS: u64 = 60;
/// The dashboard warns about a sweep not final after this many minutes.
pub const STUCK_BROADCAST_MINUTES: i32 = 30;
/// The dashboard warns about confirmed funds not swept after this many minutes.
pub const STUCK_UNSWEPT_MINUTES: i32 = 60;

/// Fraction of the amount due that counts as paid (absorbs wallet rounding).
pub fn min_due(pay_amount: Decimal) -> Decimal {
    pay_amount * Decimal::new(99, 2)
}

/// Re-run the reducer over a payment's transactions and save the result.
pub async fn recompute(app: &App, payment_id: i64) -> Result<PaymentRow> {
    let p = app.store.payment_any(payment_id).await?.ok_or(Error::NotFound)?;
    if matches!(p.status, PaymentStatus::Refunded | PaymentStatus::Failed) {
        return Ok(p);
    }
    let txs = app.store.payment_tx_views(payment_id).await?;
    if p.status == PaymentStatus::Finished {
        // The status stays, but a later deposit (overpayment, late top-up) changes the amount received.
        let paid: Decimal = txs.iter().map(|t| t.amount).sum();
        if paid == p.actually_paid || !app.store.update_finished_paid(payment_id, paid).await? {
            return Ok(p);
        }
        let updated = app.store.payment_any(payment_id).await?.ok_or(Error::NotFound)?;
        webhooks::payment_received(app, &updated).await?;
        return Ok(updated);
    }
    let network = app.store.network(p.network_id).await?;
    // Watch for late top-ups for one day after the checkout closes.
    let monitoring_until = p.expires_at + chrono::Duration::days(1);
    let r = reducer::reduce(p.pay_amount, Some(p.expires_at), Some(monitoring_until), &txs, network.confirmations_required, Utc::now());
    app.store.update_payment_detection(payment_id, r.status, r.settlement, r.actually_paid, r.confirmations).await?;
    let updated = app.store.payment_any(payment_id).await?.ok_or(Error::NotFound)?;
    if updated.status != p.status {
        app.events.publish(DomainEvent::PaymentStatusChanged { payment_id, from: p.status, to: updated.status });
        webhooks::payment_changed(app, &updated).await?;
    } else if updated.confirmations != p.confirmations && updated.status == PaymentStatus::Confirming {
        // Progress: one event per new confirmation count.
        let key = format!("payment:{payment_id}:{}:confirming:{}", updated.status_seq, updated.confirmations);
        let data = crate::payments::payment_json_full(app, &updated, crate::payments::PaymentShape::Webhook).await?;
        if let Some(url) = updated.ipn_callback_url.as_deref() {
            app.store
                .enqueue_delivery(updated.project_id, None, url, "payment.confirming", &key, false, &data, &crate::ids::token())
                .await?;
        }
    }
    Ok(updated)
}

/// Credit confirmed funds and finish the payment when they cover the amount due.
pub async fn credit(app: &App, payment_id: i64) -> Result<()> {
    let p = app.store.payment_any(payment_id).await?.ok_or(Error::NotFound)?;
    if matches!(p.status, PaymentStatus::Refunded | PaymentStatus::Failed) {
        return Ok(());
    }
    let network = app.store.network(p.network_id).await?;
    let confirmed = app.store.confirmed_amount(payment_id, network.confirmations_required).await?;
    let credited = p.outcome_amount.unwrap_or(Decimal::ZERO);
    if confirmed <= credited {
        return Ok(());
    }
    app.store.credit_payment(payment_id, confirmed - credited, Decimal::ZERO).await?;
    let finished_now = confirmed >= min_due(p.pay_amount) && app.store.mark_finished(payment_id).await?;
    let p = app.store.payment_any(payment_id).await?.ok_or(Error::NotFound)?;
    if finished_now {
        app.events.publish(DomainEvent::PaymentFinished { payment_id });
        webhooks::payment_changed(app, &p).await?;
    } else if p.status == PaymentStatus::Finished {
        webhooks::payment_credited(app, &p, &confirmed.normalize().to_string()).await?;
    }
    Ok(())
}

/// Poll the chain for one payment, then update its state and sweep what is confirmed.
pub async fn process_payment(app: &App, payment_id: i64) -> Result<()> {
    let p = app.store.payment_any(payment_id).await?.ok_or(Error::NotFound)?;
    let network = app.store.network(p.network_id).await?;
    if let Some(driver) = app.drivers().get(&network.chain) {
        let an = app.store.asset_network(p.asset_network_id).await?;
        let asset = AssetContext::from_asset_network(&an);
        let w = crate::addresses::wallet_context(app, p.project_id, p.network_id).await?;
        for o in driver.observe(&w, &p.pay_address, p.payin_extra_id.as_deref(), &asset).await? {
            app.store
                .upsert_payment_transaction(
                    payment_id,
                    p.network_id,
                    &o.chain_tx_key,
                    &o.txid,
                    o.vout,
                    o.amount,
                    o.confirmations,
                    o.block_height,
                )
                .await?;
        }
    }
    recompute(app, payment_id).await?;
    credit(app, payment_id).await?;
    confirm_sweeps(app, payment_id).await?;
    sweep(app, payment_id).await
}

/// Check the broadcast sweeps of a payment on chain. A confirmed one is final. A failed or
/// expired one gives its amount back to the payment, so that `sweep` sends it again; a sweep
/// that may still land keeps its amount, so the funds are never sent twice.
pub async fn confirm_sweeps(app: &App, payment_id: i64) -> Result<()> {
    let open: Vec<_> = app.store.sweeps_for_payment(payment_id).await?.into_iter().filter(|s| s.status == "broadcast").collect();
    if open.is_empty() {
        return Ok(());
    }
    let p = app.store.payment_any(payment_id).await?.ok_or(Error::NotFound)?;
    let network = app.store.network(p.network_id).await?;
    let Some(driver) = app.drivers().get(&network.chain) else { return Ok(()) };
    if !driver.needs_sweep() {
        return Ok(());
    }
    let asset = AssetContext::from_asset_network(&app.store.asset_network(p.asset_network_id).await?);
    for s in open {
        let Some(txid) = s.txid.clone() else { continue };
        let check = SweepCheck { txid, from_address: s.from_address.clone(), to_address: s.to_address.clone(), broadcast_at: s.created_at };
        match driver.sweep_state(&check, &asset).await {
            Ok(SweepState::Pending) => {}
            Ok(SweepState::Confirmed) => app.store.confirm_sweep(s.sweep_id).await?,
            Ok(SweepState::Failed(reason)) => {
                if app.store.fail_sweep(payment_id, s.sweep_id, &reason).await? {
                    tracing::warn!(payment_id, sweep_id = s.sweep_id, txid = %check.txid, reason, "sweep failed on chain; sending it again");
                }
            }
            Err(e) => tracing::warn!(payment_id, sweep_id = s.sweep_id, error = %e, "sweep check failed; trying later"),
        }
    }
    Ok(())
}

/// Sweeps of payments that the watcher no longer follows (finished or expired, past the
/// monitoring window): confirm broadcast ones and send funds still at the deposit address.
pub async fn sweep_backlog(app: &App) {
    let ids = match app.store.sweep_backlog_ids(pool::SEED_FAMILIES).await {
        Ok(ids) => ids,
        Err(e) => return tracing::warn!(error = %e, "listing unswept payments failed"),
    };
    for id in ids {
        let res = async {
            confirm_sweeps(app, id).await?;
            sweep(app, id).await
        };
        if let Err(e) = res.await {
            tracing::warn!(payment_id = id, error = %e, "sweep of an old payment failed");
        }
    }
}

/// Move confirmed, not yet swept funds of a payment to the project's settlement address.
pub async fn sweep(app: &App, payment_id: i64) -> Result<()> {
    let p = app.store.payment_any(payment_id).await?.ok_or(Error::NotFound)?;
    if matches!(p.status, PaymentStatus::Refunded | PaymentStatus::Failed) {
        return Ok(());
    }
    let network = app.store.network(p.network_id).await?;
    let Some(driver) = app.drivers().get(&network.chain) else { return Ok(()) };
    if !driver.needs_sweep() {
        return Ok(());
    }
    let an = app.store.asset_network(p.asset_network_id).await?;
    let confirmed = app.store.confirmed_amount(payment_id, network.confirmations_required).await?;
    let swept: Decimal =
        sqlx::query_scalar("SELECT swept_gross FROM payments WHERE payment_id = $1").bind(payment_id).fetch_one(&app.store.pool).await?;
    let available = confirmed - swept;
    if available <= an.min_payment_amount.unwrap_or(Decimal::ZERO) || available <= Decimal::ZERO {
        return Ok(());
    }
    let Some(wallet) = app.store.project_wallet(p.project_id, p.network_id).await? else { return Ok(()) };
    let Some(to) = wallet.address.clone() else { return Ok(()) };
    let Some((index, from)) = app.store.deposit_address_for_payment(payment_id).await? else { return Ok(()) };
    let (failures, last_failure) = app.store.sweep_failures(payment_id).await?;
    if failures >= QUICK_SWEEP_RETRIES && last_failure.is_some_and(|t| Utc::now() - t < chrono::Duration::minutes(SLOW_SWEEP_RETRY_MINUTES))
    {
        return Ok(());
    }
    if !app.store.reserve_forward(payment_id, available, swept).await? {
        return Ok(());
    }
    let req = SweepRequest {
        from_address: from.clone(),
        to_address: to.clone(),
        to_extra_id: wallet.address_extra_id.clone(),
        amount: available,
        derivation_index: index as u64,
    };
    let mut w = crate::addresses::wallet_context(app, p.project_id, p.network_id).await?;
    if app.hot_identity().is_some()
        && let Some(k) = crate::hot::key_for_project(app, p.project_id, &network.family).await?
    {
        w.hot_address = Some(k.address);
        w.hot_secret = Some(k.secret);
    }
    match driver.sweep(&w, &req, &AssetContext::from_asset_network(&an)).await {
        Ok(res) => {
            let net = (available - res.network_fee).max(Decimal::ZERO);
            let sweep_id = app
                .store
                .insert_sweep(
                    p.project_id,
                    p.network_id,
                    p.asset_network_id,
                    Some(payment_id),
                    &from,
                    &to,
                    net,
                    res.network_fee,
                    "broadcast",
                    Some(&res.txid),
                )
                .await?;
            app.store.credit_payment(payment_id, Decimal::ZERO, res.network_fee).await?;
            app.store.mark_deposit_swept(payment_id).await?;
            app.events.publish(DomainEvent::SweepBroadcast { sweep_id, txid: res.txid });
            Ok(())
        }
        Err(e) => {
            app.store.unreserve_forward(payment_id, available).await?;
            tracing::warn!(payment_id, error = %e, "sweep failed; retrying later");
            // A node that does not answer sent nothing. Any other error can follow a broadcast that
            // cost fees, so it counts toward the retry limit.
            if !matches!(e, Error::Unavailable(_)) {
                let msg = crate::chains::registry::redact_error(&e.to_string());
                app.store
                    .record_failed_sweep(p.project_id, p.network_id, p.asset_network_id, payment_id, &from, &to, available, &msg)
                    .await?;
            }
            Ok(())
        }
    }
}

/// One pass over every watched payment.
pub async fn tick(app: &App) {
    if let Err(e) = app.refresh_drivers().await {
        tracing::warn!(error = %e, "driver refresh failed");
    }
    match app.store.active_payment_ids().await {
        Ok(ids) => {
            for id in ids {
                if let Err(e) = process_payment(app, id).await {
                    tracing::warn!(payment_id = id, error = %e, "watch failed");
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "listing active payments failed"),
    }
    if let Err(e) = webhooks::deliver_due(app).await {
        tracing::warn!(error = %e, "webhook delivery failed");
    }
}

/// Start the background loops.
pub fn spawn(app: App) {
    crate::payouts::signer::spawn(app.clone());
    tokio::spawn(crate::chains::registry::health_loop(app.clone(), crate::chains::registry::WORKER_PROBE));
    tokio::spawn(crate::updates::run(app.clone()));
    crate::exchange::spawn(app.clone());
    #[cfg(feature = "evm")]
    evm_sweeper::spawn(app.clone());
    let a = app.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            tick(&a).await;
        }
    });
    let a = app.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(SWEEP_BACKLOG_SECONDS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            sweep_backlog(&a).await;
        }
    });
    let a = app.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(pool::REFILL_SECONDS));
        loop {
            interval.tick().await;
            pool::refill(&a).await;
        }
    });
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            let _ = app.store.delete_expired_sessions().await;
        }
    });
}
