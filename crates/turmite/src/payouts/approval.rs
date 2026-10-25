//! The emailed approval code: 6 digits, valid 10 minutes, 5 attempts, bound to one payout and one admin.
//! Only a keyed hash of the code is stored.

use chrono::{DateTime, Duration, Utc};

use crate::{app::App, crypto::ct_eq, db::ApprovalCodeRow, ids};

pub const CODE_DIGITS: u32 = 6;
pub const CODE_TTL_MINUTES: i64 = 10;
pub const MAX_ATTEMPTS: i32 = 5;

/// Keyed hash of a code, bound to the payout and the admin.
pub fn code_hash(app: &App, payout_id: i64, user_id: i64, code: &str) -> String {
    app.sealer.mac("payout_approval", format!("{payout_id}:{user_id}:{}", code.trim()).as_bytes())
}

pub fn new_code() -> String {
    ids::new_numeric_code(CODE_DIGITS)
}

pub fn expiry(now: DateTime<Utc>) -> DateTime<Utc> {
    now + Duration::minutes(CODE_TTL_MINUTES)
}

/// Result of checking an entered code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    /// Wrong code; `left` attempts remain. 0: the code is used up.
    Wrong {
        left: i32,
    },
    Expired,
    /// The code was sent to another admin.
    OtherAdmin,
}

/// Check `code` against the stored row. `hash` computes the keyed hash for (user, code).
pub fn check(row: &ApprovalCodeRow, user_id: i64, code: &str, now: DateTime<Utc>, hash: impl Fn(i64, &str) -> String) -> Verdict {
    if row.user_id != user_id {
        return Verdict::OtherAdmin;
    }
    if now >= row.expires_at || row.attempts >= MAX_ATTEMPTS {
        return Verdict::Expired;
    }
    let code = code.trim();
    let well_formed = code.len() == CODE_DIGITS as usize && code.bytes().all(|b| b.is_ascii_digit());
    if well_formed && ct_eq(hash(user_id, code).as_bytes(), row.code_hash.as_bytes()) {
        return Verdict::Ok;
    }
    Verdict::Wrong { left: (MAX_ATTEMPTS - row.attempts - 1).max(0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(user: i64, code: &str) -> String {
        format!("{user}:{code}")
    }

    fn row(attempts: i32, expires_in_secs: i64) -> ApprovalCodeRow {
        ApprovalCodeRow {
            payout_id: 1,
            user_id: 7,
            code_hash: h(7, "123456"),
            attempts,
            expires_at: Utc::now() + Duration::seconds(expires_in_secs),
        }
    }

    #[test]
    fn right_code() {
        assert_eq!(check(&row(0, 600), 7, "123456", Utc::now(), h), Verdict::Ok);
        assert_eq!(check(&row(4, 600), 7, " 123456 ", Utc::now(), h), Verdict::Ok);
    }

    #[test]
    fn wrong_code_counts_down() {
        assert_eq!(check(&row(0, 600), 7, "000000", Utc::now(), h), Verdict::Wrong { left: 4 });
        assert_eq!(check(&row(4, 600), 7, "000000", Utc::now(), h), Verdict::Wrong { left: 0 });
        assert_eq!(check(&row(0, 600), 7, "12345", Utc::now(), h), Verdict::Wrong { left: 4 });
    }

    #[test]
    fn used_up_after_five_attempts() {
        assert_eq!(check(&row(5, 600), 7, "123456", Utc::now(), h), Verdict::Expired);
    }

    #[test]
    fn expires_after_ten_minutes() {
        let now = Utc::now();
        assert_eq!(expiry(now) - now, Duration::minutes(10));
        assert_eq!(check(&row(0, -1), 7, "123456", Utc::now(), h), Verdict::Expired);
    }

    #[test]
    fn bound_to_the_admin() {
        assert_eq!(check(&row(0, 600), 8, "123456", Utc::now(), h), Verdict::OtherAdmin);
    }

    #[test]
    fn codes_are_six_digits() {
        let c = new_code();
        assert_eq!(c.len(), 6);
        assert!(c.bytes().all(|b| b.is_ascii_digit()));
    }
}
