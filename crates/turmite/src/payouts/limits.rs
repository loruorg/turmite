//! Project payout limits in USD.
//!
//! - Minimum: smaller payouts are refused.
//! - Auto-approve up to: larger payouts wait for a dashboard approval. 0: every payout waits.
//! - Hourly and daily limits (0 = none): a payout that would take the total of the window over the limit
//!   waits for a dashboard approval. The total counts every payout cleared to send in the window, in-flight
//!   ones included.

use rust_decimal::Decimal;

use crate::error::{Error, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PayoutLimits {
    pub min_usd: Decimal,
    pub auto_approve_usd: Decimal,
    pub hourly_usd: Decimal,
    pub daily_usd: Decimal,
}

/// Why a payout waits for a person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    NoRate,
    AboveAutoApprove,
    HourlyLimit,
    DailyLimit,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::NoRate => "no-rate",
            Reason::AboveAutoApprove => "above-auto-approve",
            Reason::HourlyLimit => "hourly-limit",
            Reason::DailyLimit => "daily-limit",
        }
    }

    pub fn text(code: &str) -> &'static str {
        match code {
            "no-rate" => "No USD rate was available, so the limits could not be checked.",
            "above-auto-approve" => "The amount is above the auto-approve amount.",
            "hourly-limit" => "The payout takes the last hour over the hourly limit.",
            "daily-limit" => "The payout takes the last 24 hours over the daily limit.",
            _ => "",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Auto,
    NeedsApproval(Reason),
}

impl PayoutLimits {
    /// Refuse a payout below the minimum. Without a USD value the minimum cannot be checked.
    pub fn check_minimum(&self, amount_usd: Option<Decimal>) -> Result<()> {
        if self.min_usd <= Decimal::ZERO {
            return Ok(());
        }
        match amount_usd {
            None => Err(Error::Rate("no USD rate to check the minimum payout".into())),
            Some(v) if v < self.min_usd => Err(Error::BadRequest(format!(
                "The payout is worth {} USD. The minimum payout of this project is {} USD.",
                v.round_dp(2).normalize(),
                self.min_usd.normalize()
            ))),
            Some(_) => Ok(()),
        }
    }

    /// Send at once, or wait for an approval. `used_hour` and `used_day` are the USD totals already
    /// cleared in the last hour and the last 24 hours.
    pub fn decide(&self, amount_usd: Option<Decimal>, used_hour: Decimal, used_day: Decimal) -> Decision {
        let Some(v) = amount_usd else { return Decision::NeedsApproval(Reason::NoRate) };
        if v > self.auto_approve_usd {
            return Decision::NeedsApproval(Reason::AboveAutoApprove);
        }
        if self.hourly_usd > Decimal::ZERO && used_hour + v > self.hourly_usd {
            return Decision::NeedsApproval(Reason::HourlyLimit);
        }
        if self.daily_usd > Decimal::ZERO && used_day + v > self.daily_usd {
            return Decision::NeedsApproval(Reason::DailyLimit);
        }
        Decision::Auto
    }

    pub fn validate(&self) -> Result<()> {
        for (name, v) in [
            ("minimum", self.min_usd),
            ("auto-approve amount", self.auto_approve_usd),
            ("hourly limit", self.hourly_usd),
            ("daily limit", self.daily_usd),
        ] {
            if v < Decimal::ZERO || v > Decimal::from(1_000_000_000_000i64) {
                return Err(Error::BadRequest(format!("The {name} must be between 0 and 1000000000000.")));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn limits() -> PayoutLimits {
        PayoutLimits { min_usd: d("5"), auto_approve_usd: d("100"), hourly_usd: d("300"), daily_usd: d("1000") }
    }

    #[test]
    fn minimum() {
        let l = limits();
        assert!(l.check_minimum(Some(d("5"))).is_ok());
        assert!(matches!(l.check_minimum(Some(d("4.99"))), Err(Error::BadRequest(_))));
        assert!(matches!(l.check_minimum(None), Err(Error::Rate(_))));
        let none = PayoutLimits::default();
        assert!(none.check_minimum(None).is_ok());
        assert!(none.check_minimum(Some(d("0.01"))).is_ok());
    }

    #[test]
    fn auto_approve() {
        let l = limits();
        assert_eq!(l.decide(Some(d("100")), Decimal::ZERO, Decimal::ZERO), Decision::Auto);
        assert_eq!(l.decide(Some(d("100.01")), Decimal::ZERO, Decimal::ZERO), Decision::NeedsApproval(Reason::AboveAutoApprove));
        assert_eq!(l.decide(None, Decimal::ZERO, Decimal::ZERO), Decision::NeedsApproval(Reason::NoRate));
        // 0 = nothing is approved automatically.
        let manual = PayoutLimits::default();
        assert_eq!(manual.decide(Some(d("0.01")), Decimal::ZERO, Decimal::ZERO), Decision::NeedsApproval(Reason::AboveAutoApprove));
    }

    #[test]
    fn windows_count_what_is_in_flight() {
        let l = limits();
        // 250 already cleared this hour: 50 fits, 51 does not.
        assert_eq!(l.decide(Some(d("50")), d("250"), d("250")), Decision::Auto);
        assert_eq!(l.decide(Some(d("51")), d("250"), d("250")), Decision::NeedsApproval(Reason::HourlyLimit));
        // The hour is quiet but the day is full.
        assert_eq!(l.decide(Some(d("60")), d("0"), d("950")), Decision::NeedsApproval(Reason::DailyLimit));
        // 0 = no window limit.
        let open = PayoutLimits { auto_approve_usd: d("100"), ..Default::default() };
        assert_eq!(open.decide(Some(d("100")), d("1000000"), d("1000000")), Decision::Auto);
    }

    #[test]
    fn validation() {
        assert!(limits().validate().is_ok());
        assert!(PayoutLimits { daily_usd: d("-1"), ..Default::default() }.validate().is_err());
    }
}
