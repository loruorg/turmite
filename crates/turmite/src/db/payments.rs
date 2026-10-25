use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{Row, postgres::PgRow};

use super::{Page, PgStore};
use crate::{
    domain::{PaymentStatus, Settlement},
    error::{Error, Result},
    reducer::TxView,
};

#[derive(Clone, Debug)]
pub struct InvoiceRow {
    pub invoice_id: i64,
    pub project_id: i64,
    pub token: String,
    pub order_id: Option<String>,
    pub order_description: Option<String>,
    pub price_amount: Decimal,
    pub price_currency: String,
    pub pay_currency: Option<String>,
    pub ipn_callback_url: Option<String>,
    pub success_url: Option<String>,
    pub cancel_url: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct PaymentRow {
    pub payment_id: i64,
    pub project_id: i64,
    pub invoice_id: Option<i64>,
    pub token: String,
    pub asset_network_id: i64,
    pub network_id: i64,
    pub purchase_id: String,
    pub order_id: Option<String>,
    pub order_description: Option<String>,
    pub price_amount: Decimal,
    pub price_currency: String,
    pub pay_address: String,
    pub payin_extra_id: Option<String>,
    pub pay_amount: Decimal,
    pub actually_paid: Decimal,
    pub outcome_amount: Option<Decimal>,
    pub network_fee_amount: Decimal,
    pub status: PaymentStatus,
    /// Status changes so far (webhook event keys).
    pub status_seq: i32,
    pub settlement: Settlement,
    pub confirmations: i32,
    pub ipn_callback_url: Option<String>,
    pub success_url: Option<String>,
    pub cancel_url: Option<String>,
    pub rate: Option<Decimal>,
    pub expires_at: DateTime<Utc>,
    pub paid_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Fields of a new payment.
#[derive(Clone, Debug)]
pub struct NewPayment<'a> {
    pub project_id: i64,
    pub invoice_id: Option<i64>,
    pub asset_network_id: i64,
    pub network_id: i64,
    pub deposit_address_id: Option<i64>,
    pub idempotency_key: Option<&'a str>,
    pub purchase_id: &'a str,
    pub order_id: Option<&'a str>,
    pub order_description: Option<&'a str>,
    pub price_amount: Decimal,
    pub price_currency: &'a str,
    pub pay_address: &'a str,
    pub payin_extra_id: Option<&'a str>,
    pub pay_amount: Decimal,
    pub rate: Option<Decimal>,
    pub ipn_callback_url: Option<&'a str>,
    pub success_url: Option<&'a str>,
    pub cancel_url: Option<&'a str>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default)]
pub struct PaymentFilter {
    pub status: Option<String>,
    pub order_id: Option<String>,
    pub invoice_id: Option<i64>,
    pub date_from: Option<DateTime<Utc>>,
    pub date_to: Option<DateTime<Utc>>,
    pub sort_by: Option<String>,
    pub ascending: bool,
}

#[derive(Clone, Debug)]
pub struct TxRow {
    pub txid: String,
    pub amount: Decimal,
    pub confirmations: i32,
    pub block_height: Option<i64>,
    pub seen_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct SweepRow {
    pub sweep_id: i64,
    pub payment_id: Option<i64>,
    pub from_address: String,
    pub to_address: String,
    pub amount: Decimal,
    pub network_fee_amount: Decimal,
    pub status: String,
    pub txid: Option<String>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// A payment whose funds wait at its deposit address longer than they should.
#[derive(Clone, Debug)]
pub struct StuckSweep {
    pub payment_id: i64,
    /// The oldest sweep that is broadcast but not final.
    pub broadcast_since: Option<DateTime<Utc>>,
    /// When the newest confirmed deposit that is not swept was confirmed.
    pub unswept_since: Option<DateTime<Utc>>,
    /// Error of the newest failed sweep.
    pub last_error: Option<String>,
}

const INVOICE_COLS: &str = "invoice_id, project_id, token, order_id, order_description, price_amount, price_currency, \
    pay_currency, ipn_callback_url, success_url, cancel_url, expires_at, created_at";

const PAYMENT_COLS: &str = "payment_id, project_id, invoice_id, token, asset_network_id, network_id, purchase_id, \
    order_id, order_description, price_amount, price_currency, pay_address, payin_extra_id, pay_amount, actually_paid, \
    outcome_amount, network_fee_amount, status, status_seq, settlement, confirmations, ipn_callback_url, success_url, \
    cancel_url, rate, expires_at, paid_at, created_at, updated_at";

fn invoice_from_row(r: &PgRow) -> InvoiceRow {
    InvoiceRow {
        invoice_id: r.get("invoice_id"),
        project_id: r.get("project_id"),
        token: r.get("token"),
        order_id: r.get("order_id"),
        order_description: r.get("order_description"),
        price_amount: r.get("price_amount"),
        price_currency: r.get("price_currency"),
        pay_currency: r.get("pay_currency"),
        ipn_callback_url: r.get("ipn_callback_url"),
        success_url: r.get("success_url"),
        cancel_url: r.get("cancel_url"),
        expires_at: r.get("expires_at"),
        created_at: r.get("created_at"),
    }
}

fn payment_from_row(r: &PgRow) -> Result<PaymentRow> {
    Ok(PaymentRow {
        payment_id: r.get("payment_id"),
        project_id: r.get("project_id"),
        invoice_id: r.get("invoice_id"),
        token: r.get("token"),
        asset_network_id: r.get("asset_network_id"),
        network_id: r.get("network_id"),
        purchase_id: r.get("purchase_id"),
        order_id: r.get("order_id"),
        order_description: r.get("order_description"),
        price_amount: r.get("price_amount"),
        price_currency: r.get("price_currency"),
        pay_address: r.get("pay_address"),
        payin_extra_id: r.get("payin_extra_id"),
        pay_amount: r.get("pay_amount"),
        actually_paid: r.get("actually_paid"),
        outcome_amount: r.get("outcome_amount"),
        network_fee_amount: r.get("network_fee_amount"),
        status: r.get::<String, _>("status").parse()?,
        status_seq: r.get("status_seq"),
        settlement: r.get::<String, _>("settlement").parse()?,
        confirmations: r.get("confirmations"),
        ipn_callback_url: r.get("ipn_callback_url"),
        success_url: r.get("success_url"),
        cancel_url: r.get("cancel_url"),
        rate: r.get("rate"),
        expires_at: r.get("expires_at"),
        paid_at: r.get("paid_at"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    })
}

impl PgStore {
    // ---- invoices ----------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub async fn create_invoice(
        &self,
        project_id: i64,
        token: &str,
        order_id: Option<&str>,
        order_description: Option<&str>,
        price_amount: Decimal,
        price_currency: &str,
        pay_currency: Option<&str>,
        ipn_callback_url: Option<&str>,
        success_url: Option<&str>,
        cancel_url: Option<&str>,
        expires_at: DateTime<Utc>,
    ) -> Result<InvoiceRow> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO invoices (project_id, token, order_id, order_description, price_amount, price_currency, \
               pay_currency, ipn_callback_url, success_url, cancel_url, expires_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) RETURNING {INVOICE_COLS}"
        )))
        .bind(project_id)
        .bind(token)
        .bind(order_id)
        .bind(order_description)
        .bind(price_amount)
        .bind(price_currency)
        .bind(pay_currency)
        .bind(ipn_callback_url)
        .bind(success_url)
        .bind(cancel_url)
        .bind(expires_at)
        .fetch_one(&self.pool)
        .await?;
        Ok(invoice_from_row(&row))
    }

    pub async fn invoice(&self, project_id: i64, invoice_id: i64) -> Result<Option<InvoiceRow>> {
        let row =
            sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {INVOICE_COLS} FROM invoices WHERE project_id = $1 AND invoice_id = $2")))
                .bind(project_id)
                .bind(invoice_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.as_ref().map(invoice_from_row))
    }

    pub async fn invoice_by_token(&self, token: &str) -> Result<Option<InvoiceRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {INVOICE_COLS} FROM invoices WHERE token = $1")))
            .bind(token)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(invoice_from_row))
    }

    pub async fn list_invoices(&self, project_id: i64, page: Page) -> Result<Vec<InvoiceRow>> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {INVOICE_COLS} FROM invoices WHERE project_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3"
        )))
        .bind(project_id)
        .bind(page.limit)
        .bind(page.offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(invoice_from_row).collect())
    }

    // ---- payments ----------------------------------------------------------

    /// Insert a payment and attach its deposit address in one transaction.
    pub async fn create_payment(&self, p: &NewPayment<'_>, token: &str) -> Result<PaymentRow> {
        let mut tx = self.pool.begin().await?;
        let res = sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO payments (project_id, invoice_id, token, asset_network_id, network_id, deposit_address_id, \
               idempotency_key, purchase_id, order_id, order_description, price_amount, price_currency, pay_address, \
               payin_extra_id, pay_amount, rate, ipn_callback_url, success_url, cancel_url, expires_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20) RETURNING {PAYMENT_COLS}"
        )))
        .bind(p.project_id)
        .bind(p.invoice_id)
        .bind(token)
        .bind(p.asset_network_id)
        .bind(p.network_id)
        .bind(p.deposit_address_id)
        .bind(p.idempotency_key)
        .bind(p.purchase_id)
        .bind(p.order_id)
        .bind(p.order_description)
        .bind(p.price_amount)
        .bind(p.price_currency)
        .bind(p.pay_address)
        .bind(p.payin_extra_id)
        .bind(p.pay_amount)
        .bind(p.rate)
        .bind(p.ipn_callback_url)
        .bind(p.success_url)
        .bind(p.cancel_url)
        .bind(p.expires_at)
        .fetch_one(&mut *tx)
        .await;
        let payment = match res {
            Ok(row) => payment_from_row(&row)?,
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                return Err(Error::Conflict("A payment with this idempotency key exists.".into()));
            }
            Err(e) => return Err(e.into()),
        };
        if let Some(id) = p.deposit_address_id {
            sqlx::query("UPDATE deposit_addresses SET payment_id = $1, reserved_at = now() WHERE deposit_address_id = $2")
                .bind(payment.payment_id)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(payment)
    }

    pub async fn payment(&self, project_id: i64, payment_id: i64) -> Result<Option<PaymentRow>> {
        let row =
            sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {PAYMENT_COLS} FROM payments WHERE project_id = $1 AND payment_id = $2")))
                .bind(project_id)
                .bind(payment_id)
                .fetch_optional(&self.pool)
                .await?;
        row.as_ref().map(payment_from_row).transpose()
    }

    /// Any project's payment by id (worker use).
    pub async fn payment_any(&self, payment_id: i64) -> Result<Option<PaymentRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {PAYMENT_COLS} FROM payments WHERE payment_id = $1")))
            .bind(payment_id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(payment_from_row).transpose()
    }

    pub async fn payment_by_token(&self, token: &str) -> Result<Option<PaymentRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {PAYMENT_COLS} FROM payments WHERE token = $1")))
            .bind(token)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(payment_from_row).transpose()
    }

    pub async fn payment_by_idempotency_key(&self, project_id: i64, key: &str) -> Result<Option<PaymentRow>> {
        let row =
            sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {PAYMENT_COLS} FROM payments WHERE project_id = $1 AND idempotency_key = $2")))
                .bind(project_id)
                .bind(key)
                .fetch_optional(&self.pool)
                .await?;
        row.as_ref().map(payment_from_row).transpose()
    }

    pub async fn latest_payment_for_invoice(&self, invoice_id: i64) -> Result<Option<PaymentRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {PAYMENT_COLS} FROM payments WHERE invoice_id = $1 ORDER BY created_at DESC LIMIT 1"
        )))
        .bind(invoice_id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(payment_from_row).transpose()
    }

    /// Payments of a project, filtered and paged. Returns (rows, total).
    pub async fn list_payments(&self, project_id: i64, f: &PaymentFilter, page: Page) -> Result<(Vec<PaymentRow>, i64)> {
        let sort = match f.sort_by.as_deref() {
            Some("payment_id") => "payment_id",
            Some("payment_status") => "status",
            Some("pay_address") => "pay_address",
            Some("price_amount") => "price_amount",
            Some("price_currency") => "price_currency",
            Some("pay_amount") => "pay_amount",
            Some("actually_paid") => "actually_paid",
            Some("pay_currency" | "outcome_currency") => {
                "(SELECT ticker FROM asset_network a WHERE a.asset_network_id = payments.asset_network_id)"
            }
            Some("order_id") => "order_id",
            Some("order_description") => "order_description",
            Some("purchase_id") => "purchase_id",
            Some("outcome_amount") => "outcome_amount",
            Some("updated_at") => "updated_at",
            _ => "created_at",
        };
        let dir = if f.ascending { "ASC" } else { "DESC" };
        let filter = "project_id = $1 AND ($2::text IS NULL OR status = $2) AND ($3::text IS NULL OR order_id = $3) \
             AND ($4::timestamptz IS NULL OR created_at >= $4) AND ($5::timestamptz IS NULL OR created_at <= $5) \
             AND ($6::bigint IS NULL OR invoice_id = $6)";
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {PAYMENT_COLS} FROM payments WHERE {filter} ORDER BY {sort} {dir}, payment_id {dir} LIMIT $7 OFFSET $8"
        )))
        .bind(project_id)
        .bind(&f.status)
        .bind(&f.order_id)
        .bind(f.date_from)
        .bind(f.date_to)
        .bind(f.invoice_id)
        .bind(page.limit)
        .bind(page.offset)
        .fetch_all(&self.pool)
        .await?;
        let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM payments WHERE {filter}")))
            .bind(project_id)
            .bind(&f.status)
            .bind(&f.order_id)
            .bind(f.date_from)
            .bind(f.date_to)
            .bind(f.invoice_id)
            .fetch_one(&self.pool)
            .await?;
        Ok((rows.iter().map(payment_from_row).collect::<Result<_>>()?, total))
    }

    /// Payments the watcher follows: open ones, plus final ones inside the one-day
    /// monitoring window so late deposits are still credited.
    pub async fn active_payment_ids(&self) -> Result<Vec<i64>> {
        Ok(sqlx::query_scalar(
            "SELECT payment_id FROM payments \
             WHERE status IN ('waiting','confirming','partially_paid','confirmed','sending') \
                OR (status IN ('expired','finished') AND expires_at + interval '1 day' > now())",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_payment_transaction(
        &self,
        payment_id: i64,
        network_id: i64,
        chain_tx_key: &str,
        txid: &str,
        vout: Option<i32>,
        amount: Decimal,
        confirmations: i32,
        block_height: Option<i64>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO payment_transactions (payment_id, network_id, chain_tx_key, txid, vout, amount, confirmations, \
               block_height, confirmed_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8, CASE WHEN $7 > 0 THEN now() ELSE NULL END) \
             ON CONFLICT (network_id, chain_tx_key) DO UPDATE SET confirmations = EXCLUDED.confirmations, \
               block_height = EXCLUDED.block_height, amount = EXCLUDED.amount, \
               payment_id = COALESCE(payment_transactions.payment_id, EXCLUDED.payment_id), \
               confirmed_at = CASE WHEN EXCLUDED.confirmations > 0 AND payment_transactions.confirmed_at IS NULL \
                                   THEN now() ELSE payment_transactions.confirmed_at END",
        )
        .bind(payment_id)
        .bind(network_id)
        .bind(chain_tx_key)
        .bind(txid)
        .bind(vout)
        .bind(amount)
        .bind(confirmations)
        .bind(block_height)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn payment_tx_views(&self, payment_id: i64) -> Result<Vec<TxView>> {
        let rows = sqlx::query("SELECT amount, confirmations FROM payment_transactions WHERE payment_id = $1 AND dropped_at IS NULL")
            .bind(payment_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(|r| TxView { amount: r.get("amount"), confirmations: r.get("confirmations") }).collect())
    }

    pub async fn payment_transactions(&self, payment_id: i64) -> Result<Vec<TxRow>> {
        let rows = sqlx::query(
            "SELECT txid, amount, confirmations, block_height, seen_at FROM payment_transactions \
             WHERE payment_id = $1 ORDER BY seen_at",
        )
        .bind(payment_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| TxRow {
                txid: r.get("txid"),
                amount: r.get("amount"),
                confirmations: r.get("confirmations"),
                block_height: r.get("block_height"),
                seen_at: r.get("seen_at"),
            })
            .collect())
    }

    /// Save the detection result. Never moves a final payment back.
    pub async fn update_payment_detection(
        &self,
        payment_id: i64,
        status: PaymentStatus,
        settlement: Settlement,
        actually_paid: Decimal,
        confirmations: i32,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE payments SET status = $1, settlement = $2, actually_paid = $3, confirmations = $4, updated_at = now(), \
               paid_at = CASE WHEN $1 IN ('confirmed','finished') AND paid_at IS NULL THEN now() ELSE paid_at END \
             WHERE payment_id = $5 AND status NOT IN ('sending','finished','refunded','failed')",
        )
        .bind(status.as_str())
        .bind(settlement.as_str())
        .bind(actually_paid)
        .bind(confirmations)
        .bind(payment_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Set `actually_paid` of a finished payment. Returns true when it changed.
    pub async fn update_finished_paid(&self, payment_id: i64, actually_paid: Decimal) -> Result<bool> {
        let res = sqlx::query(
            "UPDATE payments SET actually_paid = $2, updated_at = now() \
             WHERE payment_id = $1 AND status = 'finished' AND actually_paid <> $2",
        )
        .bind(payment_id)
        .bind(actually_paid)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Sum of deposits with at least `confirmations_required` confirmations.
    pub async fn confirmed_amount(&self, payment_id: i64, confirmations_required: i32) -> Result<Decimal> {
        Ok(sqlx::query_scalar(
            "SELECT COALESCE(SUM(amount), 0) FROM payment_transactions \
             WHERE payment_id = $1 AND confirmations >= $2 AND dropped_at IS NULL",
        )
        .bind(payment_id)
        .bind(confirmations_required)
        .fetch_one(&self.pool)
        .await?)
    }

    /// Claim `delta` for forwarding if `swept_gross` still equals `expected`.
    pub async fn reserve_forward(&self, payment_id: i64, delta: Decimal, expected: Decimal) -> Result<bool> {
        let res = sqlx::query(
            "UPDATE payments SET swept_gross = swept_gross + $1, updated_at = now() WHERE payment_id = $2 AND swept_gross = $3",
        )
        .bind(delta)
        .bind(payment_id)
        .bind(expected)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() == 1)
    }

    pub async fn unreserve_forward(&self, payment_id: i64, delta: Decimal) -> Result<()> {
        sqlx::query("UPDATE payments SET swept_gross = GREATEST(swept_gross - $1, 0), updated_at = now() WHERE payment_id = $2")
            .bind(delta)
            .bind(payment_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Add to the amount credited to the merchant.
    pub async fn credit_payment(&self, payment_id: i64, net: Decimal, network_fee: Decimal) -> Result<()> {
        sqlx::query(
            "UPDATE payments SET outcome_amount = COALESCE(outcome_amount, 0) + $1, \
               network_fee_amount = network_fee_amount + $2, paid_at = COALESCE(paid_at, now()), updated_at = now() \
             WHERE payment_id = $3",
        )
        .bind(net)
        .bind(network_fee)
        .bind(payment_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Returns true when this call moved the payment to `finished`.
    pub async fn mark_finished(&self, payment_id: i64) -> Result<bool> {
        let res = sqlx::query(
            "UPDATE payments SET status = 'finished', paid_at = COALESCE(paid_at, now()), updated_at = now() \
             WHERE payment_id = $1 AND status NOT IN ('refunded','failed','finished')",
        )
        .bind(payment_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Only from the statuses that `PaymentStatus::can_refund` allows.
    pub async fn mark_refunded(&self, project_id: i64, payment_id: i64) -> Result<bool> {
        let res = sqlx::query(
            "UPDATE payments SET status = 'refunded', settlement = 'refunded', updated_at = now() \
             WHERE project_id = $1 AND payment_id = $2 \
               AND (status IN ('finished','partially_paid','failed') OR (status = 'expired' AND actually_paid > 0))",
        )
        .bind(project_id)
        .bind(payment_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Mark an unpaid or underpaid payment paid by hand.
    pub async fn mark_paid_manually(&self, project_id: i64, payment_id: i64) -> Result<bool> {
        let res = sqlx::query(
            "UPDATE payments SET status = 'finished', settlement = 'marked_paid', paid_at = COALESCE(paid_at, now()), \
               updated_at = now() WHERE project_id = $1 AND payment_id = $2 AND status NOT IN ('finished','refunded')",
        )
        .bind(project_id)
        .bind(payment_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Per ticker: credited amount and amount still pending. Refunded payments do not count.
    pub async fn balances(&self, project_id: i64) -> Result<Vec<(String, Decimal, Decimal)>> {
        let rows = sqlx::query(
            "SELECT an.ticker, \
               COALESCE(SUM(CASE WHEN p.status <> 'refunded' THEN COALESCE(p.outcome_amount, 0) ELSE 0 END), 0) AS amount, \
               COALESCE(SUM(CASE WHEN p.status IN ('waiting','confirming','partially_paid','confirmed','sending') \
                 THEN p.pay_amount - COALESCE(p.outcome_amount, 0) ELSE 0 END), 0) AS pending \
             FROM payments p JOIN asset_network an ON an.asset_network_id = p.asset_network_id \
             WHERE p.project_id = $1 GROUP BY an.ticker ORDER BY an.ticker",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(|r| (r.get("ticker"), r.get("amount"), r.get("pending"))).collect())
    }

    /// Counts and USD-free totals for the overview page: (status, count).
    pub async fn payment_status_counts(&self, project_id: i64, since: DateTime<Utc>) -> Result<Vec<(String, i64)>> {
        let rows = sqlx::query("SELECT status, COUNT(*) AS n FROM payments WHERE project_id = $1 AND created_at >= $2 GROUP BY status")
            .bind(project_id)
            .bind(since)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(|r| (r.get("status"), r.get("n"))).collect())
    }

    /// Payments created and paid per UTC day, oldest first, one row for each of the last `days` days.
    pub async fn payment_daily(&self, project_id: i64, days: i32) -> Result<Vec<(DateTime<Utc>, i64, i64)>> {
        let rows = sqlx::query(
            "SELECT d.day, \
               (SELECT COUNT(*) FROM payments p WHERE p.project_id = $1 AND p.created_at >= d.day AND p.created_at < d.day + interval '1 day') AS created, \
               (SELECT COUNT(*) FROM payments p WHERE p.project_id = $1 AND p.paid_at >= d.day AND p.paid_at < d.day + interval '1 day' AND p.status <> 'refunded') AS paid \
             FROM generate_series(date_trunc('day', now(), 'UTC') - make_interval(days => $2 - 1), \
                                  date_trunc('day', now(), 'UTC'), interval '1 day') AS d(day) ORDER BY d.day",
        )
        .bind(project_id)
        .bind(days)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(|r| (r.get("day"), r.get("created"), r.get("paid"))).collect())
    }

    /// Sum of the prices of payments paid since `since`, per price currency, largest first.
    pub async fn paid_volume(&self, project_id: i64, since: DateTime<Utc>) -> Result<Vec<(String, Decimal)>> {
        let rows = sqlx::query(
            "SELECT price_currency, SUM(price_amount) AS total FROM payments \
             WHERE project_id = $1 AND paid_at >= $2 AND status <> 'refunded' GROUP BY price_currency ORDER BY total DESC",
        )
        .bind(project_id)
        .bind(since)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(|r| (r.get("price_currency"), r.get("total"))).collect())
    }

    // ---- sweeps ------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub async fn insert_sweep(
        &self,
        project_id: i64,
        network_id: i64,
        asset_network_id: i64,
        payment_id: Option<i64>,
        from: &str,
        to: &str,
        amount: Decimal,
        network_fee: Decimal,
        status: &str,
        txid: Option<&str>,
    ) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "INSERT INTO sweeps (project_id, network_id, asset_network_id, payment_id, from_address, to_address, amount, \
               network_fee_amount, status, txid) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING sweep_id",
        )
        .bind(project_id)
        .bind(network_id)
        .bind(asset_network_id)
        .bind(payment_id)
        .bind(from)
        .bind(to)
        .bind(amount)
        .bind(network_fee)
        .bind(status)
        .bind(txid)
        .fetch_one(&self.pool)
        .await?)
    }

    /// A sweep that the driver refused before it reached the chain. The row counts as a failure for
    /// the retry limit and shows the error in the dashboard; it holds no amount.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_failed_sweep(
        &self,
        project_id: i64,
        network_id: i64,
        asset_network_id: i64,
        payment_id: i64,
        from: &str,
        to: &str,
        amount: Decimal,
        error: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO sweeps (project_id, network_id, asset_network_id, payment_id, from_address, to_address, amount, \
               network_fee_amount, status, last_error) VALUES ($1,$2,$3,$4,$5,$6,$7,0,'failed',$8)",
        )
        .bind(project_id)
        .bind(network_id)
        .bind(asset_network_id)
        .bind(payment_id)
        .bind(from)
        .bind(to)
        .bind(amount)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn sweeps_for_payment(&self, payment_id: i64) -> Result<Vec<SweepRow>> {
        let rows = sqlx::query(
            "SELECT sweep_id, payment_id, from_address, to_address, amount, network_fee_amount, status, txid, last_error, created_at \
             FROM sweeps WHERE payment_id = $1 ORDER BY sweep_id",
        )
        .bind(payment_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(sweep_from_row).collect())
    }

    /// A broadcast sweep is final on chain.
    pub async fn confirm_sweep(&self, sweep_id: i64) -> Result<()> {
        sqlx::query("UPDATE sweeps SET status = 'confirmed', updated_at = now() WHERE sweep_id = $1 AND status = 'broadcast'")
            .bind(sweep_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// A broadcast sweep did not move the funds and can no longer land: mark it failed and give its
    /// amount and fee back to the payment, so that the next sweep sends them again. Returns false
    /// when the sweep was not `broadcast` any more.
    pub async fn fail_sweep(&self, payment_id: i64, sweep_id: i64, error: &str) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "UPDATE sweeps SET status = 'failed', last_error = $3, updated_at = now() \
             WHERE sweep_id = $1 AND payment_id = $2 AND status = 'broadcast' RETURNING amount, network_fee_amount",
        )
        .bind(sweep_id)
        .bind(payment_id)
        .bind(error)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else { return Ok(false) };
        let (amount, fee): (Decimal, Decimal) = (row.get("amount"), row.get("network_fee_amount"));
        sqlx::query(
            "UPDATE payments SET swept_gross = GREATEST(swept_gross - $2, 0), \
               network_fee_amount = GREATEST(network_fee_amount - $3, 0), updated_at = now() WHERE payment_id = $1",
        )
        .bind(payment_id)
        .bind(amount + fee)
        .bind(fee)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE deposit_addresses SET is_swept = false WHERE payment_id = $1").bind(payment_id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// (number, newest time) of the sweeps of a payment that failed on chain.
    pub async fn sweep_failures(&self, payment_id: i64) -> Result<(i64, Option<DateTime<Utc>>)> {
        let row = sqlx::query("SELECT COUNT(*) AS n, MAX(updated_at) AS at FROM sweeps WHERE payment_id = $1 AND status = 'failed'")
            .bind(payment_id)
            .fetch_one(&self.pool)
            .await?;
        Ok((row.get("n"), row.get("at")))
    }

    /// Payments on `families` networks that the watcher no longer follows, with a sweep that is
    /// not final or with confirmed funds above the minimum that are not swept.
    pub async fn sweep_backlog_ids(&self, families: &[&str]) -> Result<Vec<i64>> {
        Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT p.payment_id FROM payments p \
             JOIN network n ON n.network_id = p.network_id \
             JOIN asset_network an ON an.asset_network_id = p.asset_network_id \
             WHERE n.family = ANY($1) AND p.status NOT IN ('refunded', 'failed') \
               AND p.status NOT IN ('waiting','confirming','partially_paid','confirmed','sending') \
               AND p.expires_at + interval '1 day' <= now() \
               AND (EXISTS (SELECT 1 FROM sweeps s WHERE s.payment_id = p.payment_id AND s.status = 'broadcast') \
                    OR {UNSWEPT} > COALESCE(an.min_payment_amount, 0)) \
             ORDER BY p.payment_id"
        )))
        .bind(families)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Payments of a project whose funds wait at the deposit address: a sweep not final after
    /// `broadcast_minutes`, or confirmed funds on `families` networks not swept after `unswept_minutes`.
    pub async fn stuck_sweeps(
        &self,
        project_id: Option<i64>,
        families: &[&str],
        broadcast_minutes: i32,
        unswept_minutes: i32,
    ) -> Result<Vec<StuckSweep>> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT * FROM (SELECT p.payment_id, \
               (SELECT MIN(s.created_at) FROM sweeps s WHERE s.payment_id = p.payment_id AND s.status = 'broadcast') AS broadcast_since, \
               CASE WHEN n.family = ANY($2) AND p.status NOT IN ('refunded', 'failed') \
                     AND {UNSWEPT} > COALESCE(an.min_payment_amount, 0) \
                 THEN (SELECT MAX(t.confirmed_at) FROM payment_transactions t \
                       WHERE t.payment_id = p.payment_id AND t.dropped_at IS NULL) END AS unswept_since, \
               (SELECT s.last_error FROM sweeps s WHERE s.payment_id = p.payment_id AND s.status = 'failed' \
                ORDER BY s.sweep_id DESC LIMIT 1) AS last_error \
             FROM payments p \
             JOIN network n ON n.network_id = p.network_id \
             JOIN asset_network an ON an.asset_network_id = p.asset_network_id \
             WHERE ($1::bigint IS NULL OR p.project_id = $1)) x \
             WHERE x.broadcast_since < now() - make_interval(mins => $3) \
                OR x.unswept_since < now() - make_interval(mins => $4) \
             ORDER BY x.payment_id DESC LIMIT 50"
        )))
        .bind(project_id)
        .bind(families)
        .bind(broadcast_minutes)
        .bind(unswept_minutes)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| StuckSweep {
                payment_id: r.get("payment_id"),
                broadcast_since: r.get("broadcast_since"),
                unswept_since: r.get("unswept_since"),
                last_error: r.get("last_error"),
            })
            .collect())
    }
}

/// Confirmed deposits of payment `p` (network `n`) that are not swept yet.
const UNSWEPT: &str = "((SELECT COALESCE(SUM(t.amount), 0) FROM payment_transactions t \
    WHERE t.payment_id = p.payment_id AND t.confirmations >= n.confirmations_required AND t.dropped_at IS NULL) - p.swept_gross)";

fn sweep_from_row(r: &PgRow) -> SweepRow {
    SweepRow {
        sweep_id: r.get("sweep_id"),
        payment_id: r.get("payment_id"),
        from_address: r.get("from_address"),
        to_address: r.get("to_address"),
        amount: r.get("amount"),
        network_fee_amount: r.get("network_fee_amount"),
        status: r.get("status"),
        txid: r.get("txid"),
        last_error: r.get("last_error"),
        created_at: r.get("created_at"),
    }
}
