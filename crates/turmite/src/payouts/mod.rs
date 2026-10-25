//! Payouts from the project's hot wallets: limits, approval, the signer loop and one module per chain family.
//!
//! Lifecycle:
//! - API batch: `pending-otp-verification` until `POST /v1/payout/{batch}/verify` with the user's TOTP code.
//! - After verification (or creation in the dashboard, which takes a TOTP code): the limits decide.
//!   Within them: `pending`. Else `pending-approval`.
//! - Approval: a project admin clicks Approve with a TOTP code. Turmite emails that admin a code and the payout
//!   is `pending-otp-verification`. The same admin enters the code: `pending`.
//! - Signer (worker): `pending` -> `signing` (signed and stored) -> `initiated` (a node accepted it) -> `sent`
//!   (enough confirmations). `failed` when the chain refuses it, `needs-review` when the outcome is unknown.

pub mod approval;
pub mod chain;
#[cfg(feature = "evm")]
pub mod evm;
pub mod limits;
pub mod rpc;
pub mod service;
pub mod signer;
#[cfg(feature = "sol")]
pub mod sol;
#[cfg(feature = "ton")]
pub mod ton;
#[cfg(feature = "tron")]
pub mod tron;

use std::collections::HashMap;

use chrono::{DateTime, SecondsFormat, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::{
    app::App,
    db::PayoutRow,
    domain::{AssetNetwork, Network},
    error::{Error, Result},
    ids, webhooks,
};

/// Payout status. The strings are part of the public API.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum PayoutStatus {
    PendingApproval,
    PendingOtpVerification,
    Pending,
    Signing,
    Initiated,
    Sent,
    Failed,
    Rejected,
    Cancelled,
    NeedsReview,
}

impl PayoutStatus {
    pub const ALL: [PayoutStatus; 10] = [
        PayoutStatus::PendingApproval,
        PayoutStatus::PendingOtpVerification,
        PayoutStatus::Pending,
        PayoutStatus::Signing,
        PayoutStatus::Initiated,
        PayoutStatus::Sent,
        PayoutStatus::Failed,
        PayoutStatus::Rejected,
        PayoutStatus::Cancelled,
        PayoutStatus::NeedsReview,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            PayoutStatus::PendingApproval => "pending-approval",
            PayoutStatus::PendingOtpVerification => "pending-otp-verification",
            PayoutStatus::Pending => "pending",
            PayoutStatus::Signing => "signing",
            PayoutStatus::Initiated => "initiated",
            PayoutStatus::Sent => "sent",
            PayoutStatus::Failed => "failed",
            PayoutStatus::Rejected => "rejected",
            PayoutStatus::Cancelled => "cancelled",
            PayoutStatus::NeedsReview => "needs-review",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|st| st.as_str() == s)
    }

    /// No further change happens.
    pub fn is_final(self) -> bool {
        matches!(self, PayoutStatus::Sent | PayoutStatus::Failed | PayoutStatus::Rejected | PayoutStatus::Cancelled)
    }

    /// The payout is cleared to send, is being sent, or may have been sent. Hourly and daily limits count these.
    pub fn counts_for_limits(self) -> bool {
        matches!(
            self,
            PayoutStatus::Pending | PayoutStatus::Signing | PayoutStatus::Initiated | PayoutStatus::Sent | PayoutStatus::NeedsReview
        )
    }

    /// Statuses from which a project admin can cancel: nothing is signed yet.
    pub const CANCELLABLE: [PayoutStatus; 3] = [PayoutStatus::PendingApproval, PayoutStatus::PendingOtpVerification, PayoutStatus::Pending];
    /// Statuses from which a project admin can reject: the payout waits for a person.
    pub const REJECTABLE: [PayoutStatus; 2] = [PayoutStatus::PendingApproval, PayoutStatus::PendingOtpVerification];
}

impl std::fmt::Display for PayoutStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The status changes the code makes. Everything else is a bug.
pub fn can_transition(from: PayoutStatus, to: PayoutStatus) -> bool {
    use PayoutStatus::*;
    matches!(
        (from, to),
        (PendingOtpVerification, Pending | PendingApproval | Rejected | Cancelled)
            | (PendingApproval, PendingOtpVerification | Rejected | Cancelled)
            | (Pending, Signing | Cancelled | Failed)
            | (Signing, Initiated | Sent | Failed)
            | (Initiated, Sent | Failed | NeedsReview)
            | (NeedsReview, Sent | Failed)
    )
}

/// Families with hot wallets. Bitcoin and Monero have none, so they have no payouts.
pub const PAYOUT_FAMILIES: [&str; 4] = ["evm", "tron", "solana", "ton"];

pub fn family_has_payouts(family: &str) -> bool {
    PAYOUT_FAMILIES.contains(&family)
}

/// Clear error for a network without payouts.
pub fn check_family(network: &Network) -> Result<()> {
    if family_has_payouts(&network.family) {
        Ok(())
    } else {
        Err(Error::BadRequest(format!(
            "Payouts are not available on {}. Turmite holds no key for it: its funds go straight to your own wallet.",
            network.name
        )))
    }
}

/// Amount in base units (wei, lamports, sun, nanoton). Refuses more decimals than the asset has.
pub fn to_base_units(amount: Decimal, decimals: u32) -> Result<u128> {
    if amount <= Decimal::ZERO {
        return Err(Error::BadRequest("The amount must be greater than 0.".into()));
    }
    let amount = amount.normalize();
    let scale = amount.scale();
    if scale > decimals {
        return Err(Error::BadRequest(format!("The amount has more than {decimals} decimals.")));
    }
    let mantissa = u128::try_from(amount.mantissa()).map_err(|_| Error::BadRequest("The amount is too large.".into()))?;
    10u128
        .checked_pow(decimals - scale)
        .and_then(|f| mantissa.checked_mul(f))
        .ok_or_else(|| Error::BadRequest("The amount is too large.".into()))
}

/// Base units back to an amount.
pub fn from_base_units(raw: u128, decimals: u32) -> Decimal {
    let mut d = Decimal::from(raw);
    // Decimal keeps 28 digits: divide in steps so large raw values stay exact enough.
    for _ in 0..decimals {
        d /= Decimal::TEN;
    }
    d.normalize()
}

/// A decimal as a string without trailing zeros.
pub fn dec_str(d: Decimal) -> String {
    d.normalize().to_string()
}

fn ts(d: DateTime<Utc>) -> String {
    d.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Asset networks by id, and the native ticker per network, for building payout objects.
pub struct Catalog {
    by_id: HashMap<i64, AssetNetwork>,
    native: HashMap<i64, String>,
}

impl Catalog {
    pub async fn load(app: &App) -> Result<Self> {
        let all = app.store.list_asset_networks().await?;
        let native = all.iter().filter(|a| a.is_native()).map(|a| (a.network.network_id, a.ticker.clone())).collect();
        Ok(Self { by_id: all.into_iter().map(|a| (a.asset_network_id, a)).collect(), native })
    }

    pub fn get(&self, asset_network_id: i64) -> Result<&AssetNetwork> {
        self.by_id.get(&asset_network_id).ok_or(Error::NotFound)
    }

    pub fn native_ticker(&self, network_id: i64) -> &str {
        self.native.get(&network_id).map(String::as_str).unwrap_or("")
    }

    pub fn all(&self) -> impl Iterator<Item = &AssetNetwork> {
        self.by_id.values()
    }
}

/// The payout object of the API and the webhooks.
pub fn payout_json(p: &PayoutRow, catalog: &Catalog) -> Value {
    let (ticker, network) = match catalog.get(p.asset_network_id) {
        Ok(an) => (an.ticker.clone(), an.network.api_name()),
        Err(_) => (String::new(), String::new()),
    };
    json!({
        "id": p.payout_id.to_string(),
        "batch_withdrawal_id": p.batch_id.to_string(),
        "address": p.address,
        "extra_id": p.extra_id,
        "currency": ticker,
        "network": network,
        "amount": dec_str(p.amount),
        "fee": p.fee.map(dec_str),
        "fee_currency": catalog.native_ticker(p.network_id),
        "status": p.status.as_str(),
        "hash": p.txid,
        "error": p.error,
        "ipn_callback_url": p.ipn_callback_url,
        "payout_description": p.description,
        "unique_external_id": p.unique_external_id,
        "created_at": ts(p.created_at),
        "requested_at": p.requested_at.map(ts),
        "updated_at": ts(p.updated_at),
    })
}

/// `{id, withdrawals}`.
pub fn batch_json(batch_id: i64, payouts: &[PayoutRow], catalog: &Catalog) -> Value {
    json!({
        "id": batch_id.to_string(),
        "withdrawals": payouts.iter().map(|p| payout_json(p, catalog)).collect::<Vec<_>>(),
    })
}

/// Queue `payout.<status>` for the project endpoints and the payout's own `ipn_callback_url`.
pub async fn notify(app: &App, p: &PayoutRow) -> Result<()> {
    let catalog = Catalog::load(app).await?;
    let data = payout_json(p, &catalog);
    let event = format!("payout.{}", p.status.as_str());
    let key = format!("payout:{}:{}:{}", p.payout_id, p.status.as_str(), p.status_seq);
    let is_final = p.status.is_final();
    webhooks::project_event(app, p.project_id, &event, &key, is_final, &data).await?;
    if let Some(url) = p.ipn_callback_url.as_deref().filter(|u| !u.is_empty()) {
        app.store.enqueue_delivery(p.project_id, None, url, &event, &key, is_final, &data, &ids::token()).await?;
    }
    Ok(())
}

/// `notify`, logging failures: the status change is already saved.
pub async fn notify_logged(app: &App, p: &PayoutRow) {
    if let Err(e) = notify(app, p).await {
        tracing::error!(payout = p.payout_id, error = %e, "queueing the payout webhook failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PayoutStatus::*;

    #[test]
    fn status_strings_round_trip() {
        for s in PayoutStatus::ALL {
            assert_eq!(PayoutStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(PayoutStatus::parse("finished"), None);
    }

    #[test]
    fn transitions() {
        // The approval path.
        assert!(can_transition(PendingOtpVerification, Pending));
        assert!(can_transition(PendingOtpVerification, PendingApproval));
        assert!(can_transition(PendingApproval, PendingOtpVerification));
        assert!(can_transition(Pending, Signing));
        assert!(can_transition(Signing, Initiated));
        assert!(can_transition(Initiated, Sent));
        assert!(can_transition(Initiated, NeedsReview));
        assert!(can_transition(NeedsReview, Sent));
        assert!(can_transition(NeedsReview, Failed));
        // Cancel only before signing; reject only while a person decides.
        for s in PayoutStatus::ALL {
            assert_eq!(can_transition(s, Cancelled), PayoutStatus::CANCELLABLE.contains(&s), "{s}");
            assert_eq!(can_transition(s, Rejected), PayoutStatus::REJECTABLE.contains(&s), "{s}");
        }
        // Final statuses never change.
        for from in PayoutStatus::ALL.into_iter().filter(|s| s.is_final()) {
            for to in PayoutStatus::ALL {
                assert!(!can_transition(from, to), "{from} -> {to}");
            }
        }
        // No shortcut from approval to the chain.
        assert!(!can_transition(PendingApproval, Signing));
        assert!(!can_transition(PendingOtpVerification, Signing));
        assert!(!can_transition(PendingApproval, Pending));
        assert!(!can_transition(Signing, Pending));
    }

    #[test]
    fn base_units() {
        assert_eq!(to_base_units("1.5".parse().unwrap(), 18).unwrap(), 1_500_000_000_000_000_000);
        assert_eq!(to_base_units("0.000001".parse().unwrap(), 6).unwrap(), 1);
        assert_eq!(to_base_units("250.00".parse().unwrap(), 6).unwrap(), 250_000_000);
        assert!(to_base_units("0.0000001".parse().unwrap(), 6).is_err());
        assert!(to_base_units(Decimal::ZERO, 6).is_err());
        assert!(to_base_units("-1".parse().unwrap(), 6).is_err());
        assert_eq!(from_base_units(1_500_000_000_000_000_000, 18), "1.5".parse::<Decimal>().unwrap());
        assert_eq!(from_base_units(1, 9), "0.000000001".parse::<Decimal>().unwrap());
    }

    #[test]
    fn families() {
        assert!(family_has_payouts("evm"));
        assert!(family_has_payouts("ton"));
        assert!(!family_has_payouts("bitcoin"));
        assert!(!family_has_payouts("monero"));
    }
}
