use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::domain::{PaymentStatus, Settlement};

/// A confirmed-or-not on-chain credit toward a payment.
#[derive(Clone, Copy, Debug)]
pub struct TxView {
    pub amount: Decimal,
    pub confirmations: i32,
}

/// Pure reducer output (detection axis only; the sweep worker owns
/// Sending/Finished). Re-runnable and side-effect free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reduced {
    pub status: PaymentStatus,
    pub settlement: Settlement,
    /// Sum of all detected deposits, confirmed or not (`actually_paid`).
    pub actually_paid: Decimal,
    /// Sum of deposits with enough confirmations: the amount safe to credit.
    pub confirmed_amount: Decimal,
    pub confirmations: i32,
}

/// Detection state of a payment from its on-chain transactions.
///
/// Covers waiting, confirming, confirmed, partially paid and expired. The worker
/// moves a confirmed payment to finished. `expires_at` closes the checkout; a later
/// deposit is marked paid late. Underpaid or unpaid payments stay watched until
/// `monitoring_expires_at` so late top-ups complete; after it they expire.
pub fn reduce(
    pay_amount: Decimal,
    expires_at: Option<DateTime<Utc>>,
    monitoring_expires_at: Option<DateTime<Utc>>,
    txs: &[TxView],
    confirmations_required: i32,
    now: DateTime<Utc>,
) -> Reduced {
    let actually_paid: Decimal = txs.iter().map(|t| t.amount).sum();
    let confirmed_amount: Decimal = txs.iter().filter(|t| t.confirmations >= confirmations_required).map(|t| t.amount).sum();
    let confirmations = txs.iter().map(|t| t.confirmations).max().unwrap_or(0);
    let past_checkout = expires_at.map(|e| now > e).unwrap_or(false);
    let past_monitoring = monitoring_expires_at.map(|m| now > m).unwrap_or(false);

    if actually_paid.is_zero() {
        // No funds: expired once the checkout window closes (still watched until
        // monitoring window for a possible late first payment).
        let status = if past_checkout { PaymentStatus::Expired } else { PaymentStatus::Waiting };
        return Reduced { status, settlement: Settlement::None, actually_paid, confirmed_amount, confirmations };
    }

    // Tolerance for fee and rounding slippage:
    // anything within 1% of the quote counts as fully paid.
    let tol = Decimal::new(1, 2); // 0.01 = 1%
    let min_due = pay_amount * (Decimal::ONE - tol);
    let max_exact = pay_amount * (Decimal::ONE + tol);
    let underpaid = actually_paid < min_due; // not enough sent overall
    let fully_confirmed = confirmed_amount >= min_due;

    if underpaid {
        // Past the monitoring window → terminal; otherwise keep watching for top-ups.
        if past_monitoring {
            return Reduced {
                status: PaymentStatus::Expired,
                settlement: Settlement::Underpaid,
                actually_paid,
                confirmed_amount,
                confirmations,
            };
        }
        let has_pending = actually_paid > confirmed_amount;
        let status = if has_pending { PaymentStatus::Confirming } else { PaymentStatus::PartiallyPaid };
        return Reduced { status, settlement: Settlement::Underpaid, actually_paid, confirmed_amount, confirmations };
    }

    // Enough sent. Settlement axis: late dominates labeling, then overpayment.
    let overpaid = actually_paid > max_exact;
    let settlement = if past_checkout {
        Settlement::PaidLate
    } else if overpaid {
        Settlement::Overpaid
    } else {
        Settlement::None
    };
    // Confirmed only once the *confirmed* funds cover the due amount.
    let status = if fully_confirmed { PaymentStatus::Confirmed } else { PaymentStatus::Confirming };

    Reduced { status, settlement, actually_paid, confirmed_amount, confirmations }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use rust_decimal_macros::dec;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_900_000_000, 0).unwrap()
    }
    fn window() -> Option<DateTime<Utc>> {
        Some(now() + Duration::days(1)) // monitoring window still open
    }

    #[test]
    fn no_funds_waiting_then_expired() {
        let r = reduce(dec!(1), Some(now() + Duration::minutes(10)), window(), &[], 1, now());
        assert_eq!(r.status, PaymentStatus::Waiting);
        let r = reduce(dec!(1), Some(now() - Duration::minutes(1)), window(), &[], 1, now());
        assert_eq!(r.status, PaymentStatus::Expired);
    }

    #[test]
    fn full_payment_confirms() {
        let txs = [TxView { amount: dec!(1.0), confirmations: 0 }];
        assert_eq!(reduce(dec!(1), None, None, &txs, 1, now()).status, PaymentStatus::Confirming);
        let txs = [TxView { amount: dec!(1.0), confirmations: 3 }];
        let r = reduce(dec!(1), None, None, &txs, 1, now());
        assert_eq!(r.status, PaymentStatus::Confirmed);
        assert_eq!(r.settlement, Settlement::None);
    }

    #[test]
    fn underpaid_and_overpaid() {
        let txs = [TxView { amount: dec!(0.5), confirmations: 3 }];
        let r = reduce(dec!(1), None, window(), &txs, 1, now());
        assert_eq!(r.status, PaymentStatus::PartiallyPaid);
        assert_eq!(r.settlement, Settlement::Underpaid);

        let txs = [TxView { amount: dec!(1.5), confirmations: 3 }];
        let r = reduce(dec!(1), None, window(), &txs, 1, now());
        assert_eq!(r.status, PaymentStatus::Confirmed);
        assert_eq!(r.settlement, Settlement::Overpaid);
    }

    #[test]
    fn paid_late_after_expiry() {
        let txs = [TxView { amount: dec!(1.0), confirmations: 3 }];
        let r = reduce(dec!(1), Some(now() - Duration::minutes(1)), window(), &txs, 1, now());
        assert_eq!(r.status, PaymentStatus::Confirmed);
        assert_eq!(r.settlement, Settlement::PaidLate);
    }

    #[test]
    fn topup_reaches_due_after_partial() {
        // 0.5 paid (partial) then a 0.5 top-up → fully paid + confirmed.
        let partial = [TxView { amount: dec!(0.5), confirmations: 3 }];
        assert_eq!(reduce(dec!(1), None, window(), &partial, 1, now()).status, PaymentStatus::PartiallyPaid);
        let topped = [TxView { amount: dec!(0.5), confirmations: 3 }, TxView { amount: dec!(0.5), confirmations: 3 }];
        let r = reduce(dec!(1), None, window(), &topped, 1, now());
        assert_eq!(r.status, PaymentStatus::Confirmed);
        assert_eq!(r.settlement, Settlement::None);
        assert_eq!(r.actually_paid, dec!(1.0));
    }

    #[test]
    fn underpaid_past_monitoring_window_is_terminal() {
        // Underpaid AND past the monitoring window → terminal Expired (give up).
        let txs = [TxView { amount: dec!(0.5), confirmations: 3 }];
        let r = reduce(dec!(1), Some(now() - Duration::days(2)), Some(now() - Duration::minutes(1)), &txs, 1, now());
        assert_eq!(r.status, PaymentStatus::Expired);
        assert_eq!(r.settlement, Settlement::Underpaid);
    }
}
