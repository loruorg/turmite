//! Formatting shared by dashboard pages.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::{
    app::App,
    db::{PaymentRow, StuckSweep},
    domain::{AssetNetwork, PaymentStatus},
    error::Result,
    http::checkout::{fmt_amount, fmt_fiat},
};

/// CSS class of a status badge.
pub fn status_class(status: &str) -> &'static str {
    match status {
        "finished" | "delivered" | "sent" | "confirmed" => "badge-ok",
        "failed" | "expired" | "dead" | "rejected" | "refunded" => "badge-bad",
        "waiting" | "confirming" | "sending" | "pending" | "delivering" | "partially_paid" => "badge-live",
        _ => "",
    }
}

pub fn status_label(status: &str) -> String {
    match status {
        "partially_paid" => "Partly paid".into(),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default().replace(['_', '-'], " ")
        }
    }
}

/// "3 min ago", "2 h ago", or a date.
pub fn ago(t: DateTime<Utc>) -> String {
    let s = (Utc::now() - t).num_seconds();
    match s {
        i64::MIN..=44 => "just now".into(),
        45..=3599 => format!("{} min ago", (s + 30) / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        86_400..=604_799 => format!("{} d ago", s / 86_400),
        _ => t.format("%Y-%m-%d").to_string(),
    }
}

pub fn datetime(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

/// One payment row in a table.
pub struct PaymentLine {
    pub id: i64,
    pub href: String,
    pub order_id: String,
    pub price: String,
    pub pay: String,
    pub network: String,
    pub status: String,
    pub status_label: String,
    pub status_class: &'static str,
    pub created: String,
}

pub async fn payment_lines(app: &App, rows: &[PaymentRow], project_base: &str) -> Result<Vec<PaymentLine>> {
    let ans: HashMap<i64, AssetNetwork> = app.store.list_asset_networks().await?.into_iter().map(|a| (a.asset_network_id, a)).collect();
    Ok(rows
        .iter()
        .map(|p| {
            let an = ans.get(&p.asset_network_id);
            let status = p.status.as_str();
            PaymentLine {
                id: p.payment_id,
                href: format!("{project_base}/payments/{}", p.payment_id),
                order_id: p.order_id.clone().unwrap_or_default(),
                price: fmt_fiat(p.price_amount, &p.price_currency),
                pay: format!("{} {}", fmt_amount(p.pay_amount), an.map(|a| a.asset.symbol.as_str()).unwrap_or("")),
                network: an.map(|a| a.network.name.clone()).unwrap_or_default(),
                status: status.to_string(),
                status_label: status_label(status),
                status_class: status_class(status),
                created: ago(p.created_at),
            }
        })
        .collect())
}

pub fn all_statuses() -> Vec<(&'static str, String)> {
    PaymentStatus::ALL.iter().map(|s| (s.as_str(), status_label(s.as_str()))).collect()
}

/// A payment whose funds wait at its deposit address, for a warning.
pub struct StuckLine {
    pub id: i64,
    pub href: String,
    pub text: String,
}

/// Payments of a project with a sweep that is not final or funds that are not swept in time.
pub async fn stuck_lines(app: &App, project_id: i64, project_base: &str) -> Result<Vec<StuckLine>> {
    let rows = app
        .store
        .stuck_sweeps(
            Some(project_id),
            crate::worker::pool::SEED_FAMILIES,
            crate::worker::STUCK_BROADCAST_MINUTES,
            crate::worker::STUCK_UNSWEPT_MINUTES,
        )
        .await?;
    Ok(rows
        .iter()
        .map(|s| StuckLine { id: s.payment_id, href: format!("{project_base}/payments/{}", s.payment_id), text: stuck_text(s) })
        .collect())
}

fn stuck_text(s: &StuckSweep) -> String {
    match (s.broadcast_since, &s.last_error, s.unswept_since) {
        (Some(t), _, _) => format!("Sweep sent {}, not final yet.", ago(t)),
        (None, Some(e), _) => format!("Sweep failed: {e}. The worker tries again."),
        (None, None, Some(t)) => format!("Funds confirmed {}, not swept yet. Check the worker log and the hot wallet balance.", ago(t)),
        (None, None, None) => "Funds wait at the deposit address.".into(),
    }
}
