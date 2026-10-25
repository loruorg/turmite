//! Payouts, payout batches, signed attempts, the address book, limits and approval codes.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{PgConnection, Row, postgres::PgRow};

use super::{Page, PgStore};
use crate::{
    error::{Error, Result},
    payouts::{PayoutStatus, limits::PayoutLimits},
};

#[derive(Clone, Debug)]
pub struct PayoutRow {
    pub payout_id: i64,
    pub batch_id: i64,
    pub project_id: i64,
    pub asset_network_id: i64,
    pub network_id: i64,
    pub address: String,
    pub extra_id: Option<String>,
    pub amount: Decimal,
    pub amount_usd: Option<Decimal>,
    pub status: PayoutStatus,
    pub status_seq: i32,
    pub approval_reason: Option<String>,
    pub description: Option<String>,
    pub unique_external_id: Option<String>,
    pub ipn_callback_url: Option<String>,
    pub hot_wallet_id: Option<i64>,
    pub from_address: Option<String>,
    pub txid: Option<String>,
    pub fee: Option<Decimal>,
    pub confirmations: i32,
    pub error: Option<String>,
    pub resolution_note: Option<String>,
    pub created_by: Option<i64>,
    pub verified_by: Option<i64>,
    pub verified_at: Option<DateTime<Utc>>,
    pub approved_by: Option<i64>,
    pub approved_at: Option<DateTime<Utc>>,
    pub requested_at: Option<DateTime<Utc>>,
    pub refused_at: Option<DateTime<Utc>>,
    pub next_check_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One signed transaction of a payout.
#[derive(Clone, Debug)]
pub struct AttemptRow {
    pub payout_attempt_id: i64,
    pub payout_id: i64,
    pub network_id: i64,
    pub from_address: String,
    pub txid: String,
    pub raw_tx: String,
    pub nonce: Option<i64>,
    pub params: serde_json::Value,
    pub valid_until: Option<DateTime<Utc>>,
    pub broadcast_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// A signed transaction to store.
#[derive(Clone, Debug)]
pub struct NewAttempt {
    pub from_address: String,
    pub txid: String,
    pub raw_tx: String,
    pub nonce: Option<i64>,
    pub params: serde_json::Value,
    pub valid_until: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct AddressBookRow {
    pub payout_address_id: i64,
    pub project_id: i64,
    pub network_id: i64,
    pub label: String,
    pub address: String,
    pub extra_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct ApprovalCodeRow {
    pub payout_id: i64,
    pub user_id: i64,
    pub code_hash: String,
    pub attempts: i32,
    pub expires_at: DateTime<Utc>,
}

/// A payout to insert.
#[derive(Clone, Debug)]
pub struct NewPayout {
    pub asset_network_id: i64,
    pub network_id: i64,
    pub address: String,
    pub extra_id: Option<String>,
    pub amount: Decimal,
    pub amount_usd: Option<Decimal>,
    pub status: PayoutStatus,
    pub description: Option<String>,
    pub unique_external_id: Option<String>,
    pub ipn_callback_url: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct PayoutFilter {
    pub status: Option<String>,
    pub batch_id: Option<i64>,
    pub network_id: Option<i64>,
    pub date_from: Option<DateTime<Utc>>,
    pub date_to: Option<DateTime<Utc>>,
    pub ascending: bool,
}

pub const PAYOUT_COLS: &str = "payout_id, batch_id, project_id, asset_network_id, network_id, address, extra_id, amount, \
    amount_usd, status, status_seq, approval_reason, description, unique_external_id, ipn_callback_url, hot_wallet_id, \
    from_address, txid, fee, confirmations, error, resolution_note, created_by, verified_by, verified_at, approved_by, \
    approved_at, requested_at, refused_at, next_check_at, created_at, updated_at";

const ATTEMPT_COLS: &str = "payout_attempt_id, payout_id, network_id, from_address, txid, raw_tx, nonce, params, valid_until, \
    broadcast_at, last_error, created_at";

pub fn payout_from_row(r: &PgRow) -> Result<PayoutRow> {
    let status: String = r.get("status");
    Ok(PayoutRow {
        payout_id: r.get("payout_id"),
        batch_id: r.get("batch_id"),
        project_id: r.get("project_id"),
        asset_network_id: r.get("asset_network_id"),
        network_id: r.get("network_id"),
        address: r.get("address"),
        extra_id: r.get("extra_id"),
        amount: r.get("amount"),
        amount_usd: r.get("amount_usd"),
        status: PayoutStatus::parse(&status).ok_or_else(|| Error::Internal(format!("unknown payout status `{status}`")))?,
        status_seq: r.get("status_seq"),
        approval_reason: r.get("approval_reason"),
        description: r.get("description"),
        unique_external_id: r.get("unique_external_id"),
        ipn_callback_url: r.get("ipn_callback_url"),
        hot_wallet_id: r.get("hot_wallet_id"),
        from_address: r.get("from_address"),
        txid: r.get("txid"),
        fee: r.get("fee"),
        confirmations: r.get("confirmations"),
        error: r.get("error"),
        resolution_note: r.get("resolution_note"),
        created_by: r.get("created_by"),
        verified_by: r.get("verified_by"),
        verified_at: r.get("verified_at"),
        approved_by: r.get("approved_by"),
        approved_at: r.get("approved_at"),
        requested_at: r.get("requested_at"),
        refused_at: r.get("refused_at"),
        next_check_at: r.get("next_check_at"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    })
}

fn attempt_from_row(r: &PgRow) -> AttemptRow {
    AttemptRow {
        payout_attempt_id: r.get("payout_attempt_id"),
        payout_id: r.get("payout_id"),
        network_id: r.get("network_id"),
        from_address: r.get("from_address"),
        txid: r.get("txid"),
        raw_tx: r.get("raw_tx"),
        nonce: r.get("nonce"),
        params: r.get("params"),
        valid_until: r.get("valid_until"),
        broadcast_at: r.get("broadcast_at"),
        last_error: r.get("last_error"),
        created_at: r.get("created_at"),
    }
}

fn book_from_row(r: &PgRow) -> AddressBookRow {
    AddressBookRow {
        payout_address_id: r.get("payout_address_id"),
        project_id: r.get("project_id"),
        network_id: r.get("network_id"),
        label: r.get("label"),
        address: r.get("address"),
        extra_id: r.get("extra_id"),
        created_at: r.get("created_at"),
    }
}

fn status_list(statuses: &[PayoutStatus]) -> Vec<String> {
    statuses.iter().map(|s| s.as_str().to_string()).collect()
}

impl PgStore {
    // ---- settings ------------------------------------------------------------------

    pub async fn payout_limits(&self, project_id: i64) -> Result<PayoutLimits> {
        let row =
            sqlx::query("SELECT min_usd, auto_approve_usd, hourly_limit_usd, daily_limit_usd FROM payout_settings WHERE project_id = $1")
                .bind(project_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row
            .map(|r| PayoutLimits {
                min_usd: r.get("min_usd"),
                auto_approve_usd: r.get("auto_approve_usd"),
                hourly_usd: r.get("hourly_limit_usd"),
                daily_usd: r.get("daily_limit_usd"),
            })
            .unwrap_or_default())
    }

    pub async fn set_payout_limits(&self, project_id: i64, l: &PayoutLimits) -> Result<()> {
        sqlx::query(
            "INSERT INTO payout_settings (project_id, min_usd, auto_approve_usd, hourly_limit_usd, daily_limit_usd) \
             VALUES ($1,$2,$3,$4,$5) ON CONFLICT (project_id) DO UPDATE SET min_usd = EXCLUDED.min_usd, \
             auto_approve_usd = EXCLUDED.auto_approve_usd, hourly_limit_usd = EXCLUDED.hourly_limit_usd, \
             daily_limit_usd = EXCLUDED.daily_limit_usd, updated_at = now()",
        )
        .bind(project_id)
        .bind(l.min_usd)
        .bind(l.auto_approve_usd)
        .bind(l.hourly_usd)
        .bind(l.daily_usd)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// USD cleared to send since `since` (in-flight payouts included). Inside the limits lock.
    pub async fn payout_usage_usd(conn: &mut PgConnection, project_id: i64, since: DateTime<Utc>) -> Result<Decimal> {
        let counted = status_list(&PayoutStatus::ALL.into_iter().filter(|s| s.counts_for_limits()).collect::<Vec<_>>());
        let v: Option<Decimal> = sqlx::query_scalar(
            "SELECT SUM(COALESCE(amount_usd, 0)) FROM payouts WHERE project_id = $1 AND approved_at >= $2 AND status = ANY($3)",
        )
        .bind(project_id)
        .bind(since)
        .bind(counted)
        .fetch_one(&mut *conn)
        .await?;
        Ok(v.unwrap_or(Decimal::ZERO))
    }

    // ---- address book --------------------------------------------------------------

    pub async fn list_payout_addresses(&self, project_id: i64) -> Result<Vec<AddressBookRow>> {
        let rows = sqlx::query(
            "SELECT payout_address_id, project_id, network_id, label, address, extra_id, created_at FROM payout_addresses \
             WHERE project_id = $1 ORDER BY network_id, label, payout_address_id",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(book_from_row).collect())
    }

    pub async fn payout_addresses_for(&self, project_id: i64, network_id: i64, address: &str) -> Result<Vec<AddressBookRow>> {
        let rows = sqlx::query(
            "SELECT payout_address_id, project_id, network_id, label, address, extra_id, created_at FROM payout_addresses \
             WHERE project_id = $1 AND network_id = $2 AND address = $3 ORDER BY payout_address_id",
        )
        .bind(project_id)
        .bind(network_id)
        .bind(address)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(book_from_row).collect())
    }

    pub async fn add_payout_address(
        &self,
        project_id: i64,
        network_id: i64,
        label: &str,
        address: &str,
        extra_id: Option<&str>,
        created_by: Option<i64>,
    ) -> Result<i64> {
        let res = sqlx::query_scalar(
            "INSERT INTO payout_addresses (project_id, network_id, label, address, extra_id, created_by) \
             VALUES ($1,$2,$3,$4,$5,$6) RETURNING payout_address_id",
        )
        .bind(project_id)
        .bind(network_id)
        .bind(label)
        .bind(address)
        .bind(extra_id)
        .bind(created_by)
        .fetch_one(&self.pool)
        .await;
        match res {
            Ok(id) => Ok(id),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                Err(Error::Conflict("This address is already in the address book.".into()))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Delete an entry. Returns the deleted row.
    pub async fn delete_payout_address(&self, project_id: i64, id: i64) -> Result<Option<AddressBookRow>> {
        let row = sqlx::query(
            "DELETE FROM payout_addresses WHERE project_id = $1 AND payout_address_id = $2 \
             RETURNING payout_address_id, project_id, network_id, label, address, extra_id, created_at",
        )
        .bind(project_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.as_ref().map(book_from_row))
    }

    // ---- batches and payouts -------------------------------------------------------

    pub async fn insert_payout_batch(
        conn: &mut PgConnection,
        project_id: i64,
        source: &str,
        created_by: Option<i64>,
        api_key_id: Option<i64>,
    ) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "INSERT INTO payout_batches (project_id, source, created_by, api_key_id) VALUES ($1,$2,$3,$4) RETURNING batch_id",
        )
        .bind(project_id)
        .bind(source)
        .bind(created_by)
        .bind(api_key_id)
        .fetch_one(&mut *conn)
        .await?)
    }

    pub async fn insert_payout(
        conn: &mut PgConnection,
        project_id: i64,
        batch_id: i64,
        p: &NewPayout,
        created_by: Option<i64>,
    ) -> std::result::Result<PayoutRow, sqlx::Error> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO payouts (batch_id, project_id, asset_network_id, network_id, address, extra_id, amount, amount_usd, \
               status, description, unique_external_id, ipn_callback_url, created_by) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) RETURNING {PAYOUT_COLS}"
        )))
        .bind(batch_id)
        .bind(project_id)
        .bind(p.asset_network_id)
        .bind(p.network_id)
        .bind(&p.address)
        .bind(&p.extra_id)
        .bind(p.amount)
        .bind(p.amount_usd)
        .bind(p.status.as_str())
        .bind(&p.description)
        .bind(&p.unique_external_id)
        .bind(&p.ipn_callback_url)
        .bind(created_by)
        .fetch_one(&mut *conn)
        .await?;
        payout_from_row(&row).map_err(|e| sqlx::Error::Protocol(e.to_string()))
    }

    pub async fn payout(&self, project_id: i64, payout_id: i64) -> Result<Option<PayoutRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {PAYOUT_COLS} FROM payouts WHERE project_id = $1 AND payout_id = $2")))
            .bind(project_id)
            .bind(payout_id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(payout_from_row).transpose()
    }

    pub async fn payout_any(&self, payout_id: i64) -> Result<Option<PayoutRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {PAYOUT_COLS} FROM payouts WHERE payout_id = $1")))
            .bind(payout_id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(payout_from_row).transpose()
    }

    /// The payout row, locked for this transaction.
    pub async fn payout_for_update(conn: &mut PgConnection, payout_id: i64) -> Result<Option<PayoutRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {PAYOUT_COLS} FROM payouts WHERE payout_id = $1 FOR UPDATE")))
            .bind(payout_id)
            .fetch_optional(&mut *conn)
            .await?;
        row.as_ref().map(payout_from_row).transpose()
    }

    pub async fn payout_by_external_id(&self, project_id: i64, unique_external_id: &str) -> Result<Option<PayoutRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {PAYOUT_COLS} FROM payouts WHERE project_id = $1 AND unique_external_id = $2"
        )))
        .bind(project_id)
        .bind(unique_external_id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(payout_from_row).transpose()
    }

    pub async fn batch_exists(&self, project_id: i64, batch_id: i64) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM payout_batches WHERE project_id = $1 AND batch_id = $2)")
            .bind(project_id)
            .bind(batch_id)
            .fetch_one(&self.pool)
            .await?)
    }

    pub async fn batch_payouts(&self, project_id: i64, batch_id: i64) -> Result<Vec<PayoutRow>> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {PAYOUT_COLS} FROM payouts WHERE project_id = $1 AND batch_id = $2 ORDER BY payout_id"
        )))
        .bind(project_id)
        .bind(batch_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(payout_from_row).collect()
    }

    pub async fn list_payouts(&self, project_id: i64, f: &PayoutFilter, page: Page) -> Result<(Vec<PayoutRow>, i64)> {
        let mut conds = vec!["project_id = $1".to_string()];
        let mut n = 1;
        let mut next = || {
            n += 1;
            format!("${n}")
        };
        if f.status.is_some() {
            conds.push(format!("status = {}", next()));
        }
        if f.batch_id.is_some() {
            conds.push(format!("batch_id = {}", next()));
        }
        if f.network_id.is_some() {
            conds.push(format!("network_id = {}", next()));
        }
        if f.date_from.is_some() {
            conds.push(format!("created_at >= {}", next()));
        }
        if f.date_to.is_some() {
            conds.push(format!("created_at <= {}", next()));
        }
        let where_sql = conds.join(" AND ");
        let order = if f.ascending { "ASC" } else { "DESC" };
        let list_sql = format!(
            "SELECT {PAYOUT_COLS}, COUNT(*) OVER () AS total FROM payouts WHERE {where_sql} \
             ORDER BY created_at {order}, payout_id {order} LIMIT {} OFFSET {}",
            page.limit, page.offset
        );
        let mut q = sqlx::query(sqlx::AssertSqlSafe(list_sql)).bind(project_id);
        if let Some(s) = &f.status {
            q = q.bind(s);
        }
        if let Some(b) = f.batch_id {
            q = q.bind(b);
        }
        if let Some(nid) = f.network_id {
            q = q.bind(nid);
        }
        if let Some(d) = f.date_from {
            q = q.bind(d);
        }
        if let Some(d) = f.date_to {
            q = q.bind(d);
        }
        let rows = q.fetch_all(&self.pool).await?;
        let total = rows.first().map(|r| r.get::<i64, _>("total")).unwrap_or(0);
        let total = if rows.is_empty() && page.offset > 0 {
            let count_sql = format!("SELECT COUNT(*) FROM payouts WHERE {where_sql}");
            let mut q = sqlx::query_scalar(sqlx::AssertSqlSafe(count_sql)).bind(project_id);
            if let Some(s) = &f.status {
                q = q.bind(s);
            }
            if let Some(b) = f.batch_id {
                q = q.bind(b);
            }
            if let Some(nid) = f.network_id {
                q = q.bind(nid);
            }
            if let Some(d) = f.date_from {
                q = q.bind(d);
            }
            if let Some(d) = f.date_to {
                q = q.bind(d);
            }
            q.fetch_one(&self.pool).await?
        } else {
            total
        };
        Ok((rows.iter().map(payout_from_row).collect::<Result<_>>()?, total))
    }

    pub async fn payout_status_counts(&self, project_id: i64) -> Result<Vec<(String, i64)>> {
        let rows = sqlx::query("SELECT status, COUNT(*) AS n FROM payouts WHERE project_id = $1 GROUP BY status")
            .bind(project_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(|r| (r.get("status"), r.get("n"))).collect())
    }

    /// Change the status when it is one of `from`. Returns the new row, or None when the status was another one.
    pub async fn set_payout_status(
        conn: &mut PgConnection,
        payout_id: i64,
        from: &[PayoutStatus],
        to: PayoutStatus,
        error: Option<&str>,
    ) -> Result<Option<PayoutRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE payouts SET status = $3, status_seq = status_seq + 1, error = COALESCE($4, error), updated_at = now(), \
               next_check_at = now() \
             WHERE payout_id = $1 AND status = ANY($2) RETURNING {PAYOUT_COLS}"
        )))
        .bind(payout_id)
        .bind(status_list(from))
        .bind(to.as_str())
        .bind(error)
        .fetch_optional(&mut *conn)
        .await?;
        row.as_ref().map(payout_from_row).transpose()
    }

    /// Clear to send, or hold for approval, after verification or creation.
    pub async fn set_payout_cleared(
        conn: &mut PgConnection,
        payout_id: i64,
        from: &[PayoutStatus],
        to: PayoutStatus,
        reason: Option<&str>,
        verified_by: Option<i64>,
    ) -> Result<Option<PayoutRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE payouts SET status = $3, status_seq = status_seq + 1, approval_reason = $4, \
               verified_by = COALESCE($5, verified_by), verified_at = COALESCE(verified_at, now()), \
               approved_at = CASE WHEN $3 = 'pending' THEN now() ELSE approved_at END, \
               updated_at = now(), next_check_at = now() \
             WHERE payout_id = $1 AND status = ANY($2) RETURNING {PAYOUT_COLS}"
        )))
        .bind(payout_id)
        .bind(status_list(from))
        .bind(to.as_str())
        .bind(reason)
        .bind(verified_by)
        .fetch_optional(&mut *conn)
        .await?;
        row.as_ref().map(payout_from_row).transpose()
    }

    /// Approved by a dashboard admin: `pending`.
    pub async fn set_payout_approved(conn: &mut PgConnection, payout_id: i64, user_id: i64) -> Result<Option<PayoutRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE payouts SET status = 'pending', status_seq = status_seq + 1, approved_by = $2, approved_at = now(), \
               updated_at = now(), next_check_at = now() \
             WHERE payout_id = $1 AND status = 'pending-otp-verification' RETURNING {PAYOUT_COLS}"
        )))
        .bind(payout_id)
        .bind(user_id)
        .fetch_optional(&mut *conn)
        .await?;
        row.as_ref().map(payout_from_row).transpose()
    }

    /// Final decision on a `needs-review` payout.
    pub async fn resolve_payout(&self, project_id: i64, payout_id: i64, sent: bool, note: &str) -> Result<Option<PayoutRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE payouts SET status = $3, status_seq = status_seq + 1, resolution_note = $4, \
               error = CASE WHEN $3 = 'failed' THEN COALESCE(error, 'Marked as not sent.') ELSE error END, updated_at = now() \
             WHERE project_id = $1 AND payout_id = $2 AND status = 'needs-review' RETURNING {PAYOUT_COLS}"
        )))
        .bind(project_id)
        .bind(payout_id)
        .bind(if sent { "sent" } else { "failed" })
        .bind(note)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(payout_from_row).transpose()
    }

    // ---- signer ----------------------------------------------------------------------

    /// Ids of payouts the signer should look at now.
    pub async fn due_payout_ids(&self, statuses: &[PayoutStatus], limit: i64) -> Result<Vec<i64>> {
        Ok(sqlx::query_scalar(
            "SELECT payout_id FROM payouts WHERE status = ANY($1) AND next_check_at <= now() ORDER BY payout_id LIMIT $2",
        )
        .bind(status_list(statuses))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Lock a `pending` payout for signing; None when another worker has it or it changed.
    pub async fn claim_pending_payout(conn: &mut PgConnection, payout_id: i64) -> Result<Option<PayoutRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {PAYOUT_COLS} FROM payouts WHERE payout_id = $1 AND status = 'pending' FOR UPDATE SKIP LOCKED"
        )))
        .bind(payout_id)
        .fetch_optional(&mut *conn)
        .await?;
        row.as_ref().map(payout_from_row).transpose()
    }

    /// Lock an in-flight payout; None when another worker has it.
    pub async fn lock_payout(conn: &mut PgConnection, payout_id: i64) -> Result<Option<PayoutRow>> {
        let row =
            sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {PAYOUT_COLS} FROM payouts WHERE payout_id = $1 FOR UPDATE SKIP LOCKED")))
                .bind(payout_id)
                .fetch_optional(&mut *conn)
                .await?;
        row.as_ref().map(payout_from_row).transpose()
    }

    pub async fn insert_payout_attempt(conn: &mut PgConnection, payout_id: i64, network_id: i64, a: &NewAttempt) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "INSERT INTO payout_attempts (payout_id, network_id, from_address, txid, raw_tx, nonce, params, valid_until) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING payout_attempt_id",
        )
        .bind(payout_id)
        .bind(network_id)
        .bind(&a.from_address)
        .bind(&a.txid)
        .bind(&a.raw_tx)
        .bind(a.nonce)
        .bind(&a.params)
        .bind(a.valid_until)
        .fetch_one(&mut *conn)
        .await?)
    }

    /// Record a signed transaction on the payout: `signing` for the first one.
    pub async fn set_payout_signed(
        conn: &mut PgConnection,
        payout_id: i64,
        hot_wallet_id: i64,
        from_address: &str,
        txid: &str,
        first: bool,
    ) -> Result<PayoutRow> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE payouts SET hot_wallet_id = $2, from_address = $3, txid = $4, \
               status = CASE WHEN $5 THEN 'signing' ELSE status END, \
               status_seq = status_seq + CASE WHEN $5 THEN 1 ELSE 0 END, \
               refused_at = NULL, error = NULL, updated_at = now(), next_check_at = now() \
             WHERE payout_id = $1 RETURNING {PAYOUT_COLS}"
        )))
        .bind(payout_id)
        .bind(hot_wallet_id)
        .bind(from_address)
        .bind(txid)
        .bind(first)
        .fetch_one(&mut *conn)
        .await?;
        payout_from_row(&row)
    }

    pub async fn payout_attempts(&self, payout_id: i64) -> Result<Vec<AttemptRow>> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {ATTEMPT_COLS} FROM payout_attempts WHERE payout_id = $1 ORDER BY payout_attempt_id"
        )))
        .bind(payout_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(attempt_from_row).collect())
    }

    pub async fn mark_attempt_broadcast(&self, attempt_id: i64) -> Result<()> {
        sqlx::query(
            "UPDATE payout_attempts SET broadcast_at = COALESCE(broadcast_at, now()), last_error = NULL WHERE payout_attempt_id = $1",
        )
        .bind(attempt_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_attempt_error(&self, attempt_id: i64, error: &str) -> Result<()> {
        sqlx::query("UPDATE payout_attempts SET last_error = $2 WHERE payout_attempt_id = $1")
            .bind(attempt_id)
            .bind(error)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The nonce for the next transaction of an EVM hot address: the node's pending nonce, or one
    /// more than the highest nonce that a payout or a sweep transaction that may still land holds.
    /// Payouts, sweeps and deploys all take it here, under [`PgStore::lock_evm_nonce`].
    pub async fn next_evm_nonce(conn: &mut PgConnection, network_id: i64, from_address: &str, pending: u64) -> Result<u64> {
        let stored: Option<i64> = sqlx::query_scalar(
            "SELECT GREATEST( \
               (SELECT MAX(a.nonce) FROM payout_attempts a JOIN payouts p ON p.payout_id = a.payout_id \
                WHERE a.network_id = $1 AND LOWER(a.from_address) = LOWER($2) \
                  AND p.status IN ('signing', 'initiated', 'needs-review')), \
               (SELECT MAX(nonce) FROM evm_txs \
                WHERE network_id = $1 AND LOWER(from_address) = LOWER($2) AND status = 'broadcast'))",
        )
        .bind(network_id)
        .bind(from_address)
        .fetch_one(&mut *conn)
        .await?;
        Ok(stored.map(|n| n as u64 + 1).unwrap_or(0).max(pending))
    }

    /// Highest nonce (EVM) or seqno (TON) stored for an address on a network by a payout that may still land.
    pub async fn max_open_nonce(conn: &mut PgConnection, network_id: i64, from_address: &str) -> Result<Option<i64>> {
        Ok(sqlx::query_scalar(
            "SELECT MAX(a.nonce) FROM payout_attempts a JOIN payouts p ON p.payout_id = a.payout_id \
             WHERE a.network_id = $1 AND LOWER(a.from_address) = LOWER($2) \
               AND p.status IN ('signing', 'initiated', 'needs-review')",
        )
        .bind(network_id)
        .bind(from_address)
        .fetch_one(&mut *conn)
        .await?)
    }

    /// A node accepted the transaction: `initiated`.
    pub async fn set_payout_initiated(&self, payout_id: i64, txid: &str) -> Result<Option<PayoutRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE payouts SET status = 'initiated', status_seq = status_seq + 1, txid = $2, \
               requested_at = COALESCE(requested_at, now()), refused_at = NULL, error = NULL, updated_at = now(), \
               next_check_at = now() + interval '5 seconds' \
             WHERE payout_id = $1 AND status = 'signing' RETURNING {PAYOUT_COLS}"
        )))
        .bind(payout_id)
        .bind(txid)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(payout_from_row).transpose()
    }

    /// Every node refused the stored transaction. Keeps the first time.
    pub async fn set_payout_refused(&self, payout_id: i64, error: &str, retry_in_secs: i64) -> Result<()> {
        sqlx::query(
            "UPDATE payouts SET refused_at = COALESCE(refused_at, now()), error = $2, updated_at = now(), \
               next_check_at = now() + make_interval(secs => $3) WHERE payout_id = $1",
        )
        .bind(payout_id)
        .bind(error)
        .bind(retry_in_secs as f64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Progress on the chain without a status change.
    pub async fn set_payout_progress(&self, payout_id: i64, txid: &str, confirmations: i32, retry_in_secs: i64) -> Result<()> {
        sqlx::query(
            "UPDATE payouts SET txid = $2, confirmations = $3, refused_at = NULL, \
               updated_at = CASE WHEN confirmations <> $3 OR txid IS DISTINCT FROM $2 THEN now() ELSE updated_at END, \
               next_check_at = now() + make_interval(secs => $4) WHERE payout_id = $1",
        )
        .bind(payout_id)
        .bind(txid)
        .bind(confirmations.max(0))
        .bind(retry_in_secs as f64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Look again later. `error` is shown on the payout but does not end it.
    pub async fn defer_payout(&self, payout_id: i64, error: Option<&str>, retry_in_secs: i64) -> Result<()> {
        sqlx::query(
            "UPDATE payouts SET error = COALESCE($2, error), next_check_at = now() + make_interval(secs => $3) WHERE payout_id = $1",
        )
        .bind(payout_id)
        .bind(error)
        .bind(retry_in_secs as f64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Final on-chain result: `sent` or `failed`, with the transaction and fee.
    #[allow(clippy::too_many_arguments)]
    pub async fn finish_payout(
        &self,
        payout_id: i64,
        from: &[PayoutStatus],
        to: PayoutStatus,
        txid: Option<&str>,
        fee: Option<Decimal>,
        confirmations: i32,
        error: Option<&str>,
    ) -> Result<Option<PayoutRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE payouts SET status = $3, status_seq = status_seq + 1, txid = COALESCE($4, txid), fee = COALESCE($5, fee), \
               confirmations = GREATEST(confirmations, $6), error = $7, updated_at = now(), \
               requested_at = CASE WHEN $3 = 'sent' THEN COALESCE(requested_at, now()) ELSE requested_at END \
             WHERE payout_id = $1 AND status = ANY($2) RETURNING {PAYOUT_COLS}"
        )))
        .bind(payout_id)
        .bind(status_list(from))
        .bind(to.as_str())
        .bind(txid)
        .bind(fee)
        .bind(confirmations.max(0))
        .bind(error)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(payout_from_row).transpose()
    }

    // ---- approval codes --------------------------------------------------------------

    pub async fn put_approval_code(
        conn: &mut PgConnection,
        payout_id: i64,
        user_id: i64,
        code_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO payout_approval_codes (payout_id, user_id, code_hash, attempts, expires_at) VALUES ($1,$2,$3,0,$4) \
             ON CONFLICT (payout_id) DO UPDATE SET user_id = EXCLUDED.user_id, code_hash = EXCLUDED.code_hash, attempts = 0, \
               expires_at = EXCLUDED.expires_at, created_at = now()",
        )
        .bind(payout_id)
        .bind(user_id)
        .bind(code_hash)
        .bind(expires_at)
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    pub async fn approval_code_for_update(conn: &mut PgConnection, payout_id: i64) -> Result<Option<ApprovalCodeRow>> {
        let row = sqlx::query(
            "SELECT payout_id, user_id, code_hash, attempts, expires_at FROM payout_approval_codes WHERE payout_id = $1 FOR UPDATE",
        )
        .bind(payout_id)
        .fetch_optional(&mut *conn)
        .await?;
        Ok(row.map(|r| ApprovalCodeRow {
            payout_id: r.get("payout_id"),
            user_id: r.get("user_id"),
            code_hash: r.get("code_hash"),
            attempts: r.get("attempts"),
            expires_at: r.get("expires_at"),
        }))
    }

    pub async fn approval_code(&self, payout_id: i64) -> Result<Option<ApprovalCodeRow>> {
        let row = sqlx::query("SELECT payout_id, user_id, code_hash, attempts, expires_at FROM payout_approval_codes WHERE payout_id = $1")
            .bind(payout_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| ApprovalCodeRow {
            payout_id: r.get("payout_id"),
            user_id: r.get("user_id"),
            code_hash: r.get("code_hash"),
            attempts: r.get("attempts"),
            expires_at: r.get("expires_at"),
        }))
    }

    pub async fn bump_approval_attempts(conn: &mut PgConnection, payout_id: i64) -> Result<()> {
        sqlx::query("UPDATE payout_approval_codes SET attempts = attempts + 1 WHERE payout_id = $1")
            .bind(payout_id)
            .execute(&mut *conn)
            .await?;
        Ok(())
    }

    pub async fn delete_approval_code(conn: &mut PgConnection, payout_id: i64) -> Result<()> {
        sqlx::query("DELETE FROM payout_approval_codes WHERE payout_id = $1").bind(payout_id).execute(&mut *conn).await?;
        Ok(())
    }

    /// Payouts whose emailed code expired, back to `pending-approval`. Returns the changed rows.
    pub async fn expire_approval_codes(&self) -> Result<Vec<PayoutRow>> {
        let mut tx = self.pool.begin().await?;
        let ids: Vec<i64> = sqlx::query_scalar("DELETE FROM payout_approval_codes WHERE expires_at < now() RETURNING payout_id")
            .fetch_all(&mut *tx)
            .await?;
        let mut out = Vec::new();
        for id in ids {
            if let Some(p) =
                Self::set_payout_status(&mut tx, id, &[PayoutStatus::PendingOtpVerification], PayoutStatus::PendingApproval, None).await?
            {
                out.push(p);
            }
        }
        tx.commit().await?;
        Ok(out)
    }

    /// Hot wallet balance check helper: does any payout of this wallet still wait on the chain?
    pub async fn in_flight_from(&self, network_id: i64, from_address: &str, except_payout: i64) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM payouts WHERE network_id = $1 AND LOWER(from_address) = LOWER($2) \
               AND status IN ('signing', 'initiated') AND payout_id <> $3)",
        )
        .bind(network_id)
        .bind(from_address)
        .bind(except_payout)
        .fetch_one(&self.pool)
        .await?)
    }
}
