//! EVM deposit contracts: salt counter, deposit addresses, watch state, hot-wallet transactions, sweeps.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{PgConnection, Row, postgres::PgRow};

use super::PgStore;
use crate::error::Result;

#[derive(Clone, Debug)]
pub struct EvmDepositRow {
    pub factory: String,
    pub cold_wallet: String,
    pub salt_index: i64,
    pub from_block: i64,
}

#[derive(Clone, Debug)]
pub struct EvmScanRow {
    pub final_to: i64,
    /// Base units, decimal string.
    pub final_flushed: String,
    pub arrivals: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct EvmTxRow {
    pub evm_tx_id: i64,
    pub network_id: i64,
    pub project_id: Option<i64>,
    pub kind: String,
    pub factory: String,
    pub cold_wallet: String,
    pub from_address: Option<String>,
    pub txid: Option<String>,
    pub salts: i32,
    pub gas_used: Option<i64>,
    pub fee_wei: Option<String>,
    pub status: String,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A payment with credited funds that are not swept yet.
#[derive(Clone, Debug)]
pub struct EvmSweepCandidate {
    pub payment_id: i64,
    pub project_id: i64,
    pub asset_network_id: i64,
    /// Credited and not swept, in asset units.
    pub available: Decimal,
    pub paid_at: Option<DateTime<Utc>>,
    pub contract_address: Option<String>,
    pub decimals: i32,
    pub coingecko_id: Option<String>,
    pub deposit: String,
    pub factory: String,
    pub cold_wallet: String,
    pub salt_index: i64,
}

/// A sweep row of a flush transaction.
#[derive(Clone, Debug)]
pub struct EvmSweepRow {
    pub sweep_id: i64,
    pub payment_id: Option<i64>,
    pub from_address: String,
    pub contract_address: Option<String>,
    pub decimals: i32,
}

const TX_COLS: &str = "evm_tx_id, network_id, project_id, kind, factory, cold_wallet, from_address, txid, salts, gas_used, \
    fee_wei::text AS fee_wei, status, last_error, created_at, updated_at";

fn tx_from_row(r: &PgRow) -> EvmTxRow {
    EvmTxRow {
        evm_tx_id: r.get("evm_tx_id"),
        network_id: r.get("network_id"),
        project_id: r.get("project_id"),
        kind: r.get("kind"),
        factory: r.get("factory"),
        cold_wallet: r.get("cold_wallet"),
        from_address: r.get("from_address"),
        txid: r.get("txid"),
        salts: r.get("salts"),
        gas_used: r.get("gas_used"),
        fee_wei: r.get("fee_wei"),
        status: r.get("status"),
        last_error: r.get("last_error"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

impl PgStore {
    /// Allocate the next install-wide index of a chain family (starts at 1).
    pub async fn next_family_index(&self, family: &str) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "INSERT INTO family_address_counter (family, next_index) VALUES ($1, 2) \
             ON CONFLICT (family) DO UPDATE SET next_index = family_address_counter.next_index + 1 \
             RETURNING next_index - 1",
        )
        .bind(family)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn insert_evm_deposit(
        &self,
        network_id: i64,
        address: &str,
        factory: &str,
        cold_wallet: &str,
        salt_index: i64,
        from_block: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO evm_deposits (network_id, address, factory, cold_wallet, salt_index, from_block) \
             VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (network_id, address) DO NOTHING",
        )
        .bind(network_id)
        .bind(address)
        .bind(factory)
        .bind(cold_wallet)
        .bind(salt_index)
        .bind(from_block)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn evm_deposit(&self, network_id: i64, address: &str) -> Result<Option<EvmDepositRow>> {
        let row =
            sqlx::query("SELECT factory, cold_wallet, salt_index, from_block FROM evm_deposits WHERE network_id = $1 AND address = $2")
                .bind(network_id)
                .bind(address)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|r| EvmDepositRow {
            factory: r.get("factory"),
            cold_wallet: r.get("cold_wallet"),
            salt_index: r.get("salt_index"),
            from_block: r.get("from_block"),
        }))
    }

    pub async fn evm_scan(&self, network_id: i64, address: &str, asset: &str) -> Result<Option<EvmScanRow>> {
        let row = sqlx::query(
            "SELECT final_to, final_flushed::text AS final_flushed, arrivals FROM evm_scans \
             WHERE network_id = $1 AND address = $2 AND asset = $3",
        )
        .bind(network_id)
        .bind(address)
        .bind(asset)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| EvmScanRow { final_to: r.get("final_to"), final_flushed: r.get("final_flushed"), arrivals: r.get("arrivals") }))
    }

    pub async fn save_evm_scan(
        &self,
        network_id: i64,
        address: &str,
        asset: &str,
        final_to: i64,
        final_flushed: &str,
        arrivals: &serde_json::Value,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO evm_scans (network_id, address, asset, final_to, final_flushed, arrivals) \
             VALUES ($1,$2,$3,$4,$5::numeric,$6) ON CONFLICT (network_id, address, asset) DO UPDATE SET \
               final_to = EXCLUDED.final_to, final_flushed = EXCLUDED.final_flushed, arrivals = EXCLUDED.arrivals, \
               updated_at = now()",
        )
        .bind(network_id)
        .bind(address)
        .bind(asset)
        .bind(final_to)
        .bind(final_flushed)
        .bind(arrivals)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Set the cold wallet and its factory on every EVM network of a project. New rows are on;
    /// existing rows keep their on/off state. Returns the network ids.
    pub async fn set_evm_cold_wallet(&self, project_id: i64, cold_wallet: &str, factory: &str) -> Result<Vec<i64>> {
        Ok(sqlx::query_scalar(
            "INSERT INTO project_wallets (project_id, network_id, address, factory_address) \
             SELECT $1, network_id, $2, $3 FROM network WHERE family = 'evm' \
             ON CONFLICT (project_id, network_id) DO UPDATE SET address = EXCLUDED.address, \
               factory_address = EXCLUDED.factory_address, updated_at = now() \
             RETURNING network_id",
        )
        .bind(project_id)
        .bind(cold_wallet)
        .bind(factory)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Queue a factory deployment for the worker. False when one is queued or in flight.
    pub async fn queue_evm_deploy(
        &self,
        network_id: i64,
        project_id: i64,
        factory: &str,
        cold_wallet: &str,
        requested_by: Option<i64>,
    ) -> Result<bool> {
        let res = sqlx::query(
            "INSERT INTO evm_txs (network_id, project_id, kind, factory, cold_wallet, status, requested_by) \
             SELECT $1, $2, 'deploy', $3, $4, 'queued', $5 WHERE NOT EXISTS ( \
               SELECT 1 FROM evm_txs WHERE network_id = $1 AND factory = $3 AND kind = 'deploy' \
                 AND status IN ('queued', 'broadcast'))",
        )
        .bind(network_id)
        .bind(project_id)
        .bind(factory)
        .bind(cold_wallet)
        .bind(requested_by)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Queued and broadcast transactions of a network, oldest first.
    pub async fn open_evm_txs(&self, network_id: i64) -> Result<Vec<EvmTxRow>> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {TX_COLS} FROM evm_txs WHERE network_id = $1 AND status IN ('queued', 'broadcast') ORDER BY evm_tx_id"
        )))
        .bind(network_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(tx_from_row).collect())
    }

    /// Latest transactions for a factory on any network.
    pub async fn recent_evm_txs(&self, factory: &str, limit: i64) -> Result<Vec<EvmTxRow>> {
        let rows =
            sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {TX_COLS} FROM evm_txs WHERE factory = $1 ORDER BY evm_tx_id DESC LIMIT $2")))
                .bind(factory)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.iter().map(tx_from_row).collect())
    }

    pub async fn evm_tx_by_txid(&self, network_id: i64, txid: &str) -> Result<Option<EvmTxRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {TX_COLS} FROM evm_txs WHERE network_id = $1 AND txid = $2")))
            .bind(network_id)
            .bind(txid)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(tx_from_row))
    }

    /// True when a deployment of this factory is queued or broadcast on the network. A
    /// queued deployment that failed to send stays queued and is sent again: a sweep must
    /// not deploy the factory a second time meanwhile.
    pub async fn evm_deploy_in_flight(&self, network_id: i64, factory: &str) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM evm_txs WHERE network_id = $1 AND factory = $2 AND kind = 'deploy' \
               AND status IN ('queued', 'broadcast'))",
        )
        .bind(network_id)
        .bind(factory)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn finish_evm_tx(
        &self,
        evm_tx_id: i64,
        status: &str,
        gas_used: Option<i64>,
        fee_wei: Option<&str>,
        last_error: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE evm_txs SET status = $2, gas_used = COALESCE($3, gas_used), fee_wei = COALESCE($4::numeric, fee_wei), \
               last_error = $5, updated_at = now() WHERE evm_tx_id = $1",
        )
        .bind(evm_tx_id)
        .bind(status)
        .bind(gas_used)
        .bind(fee_wei)
        .bind(last_error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Payments on a network whose credited funds are not swept, without a sweep in flight or a
    /// sweep that failed in the last hour. Oldest first.
    pub async fn evm_sweep_candidates(&self, network_id: i64, project_id: Option<i64>) -> Result<Vec<EvmSweepCandidate>> {
        let rows = sqlx::query(
            "SELECT p.payment_id, p.project_id, p.asset_network_id, p.paid_at, \
               COALESCE(p.outcome_amount, 0) - p.swept_gross AS available, \
               an.contract_address, an.decimals, a.coingecko_id, da.address AS deposit, \
               d.factory, d.cold_wallet, d.salt_index \
             FROM payments p \
             JOIN asset_network an ON an.asset_network_id = p.asset_network_id \
             JOIN asset a ON a.asset_id = an.asset_id \
             JOIN deposit_addresses da ON da.payment_id = p.payment_id \
             JOIN evm_deposits d ON d.network_id = p.network_id AND d.address = LOWER(da.address) \
             WHERE p.network_id = $1 AND ($2::bigint IS NULL OR p.project_id = $2) \
               AND COALESCE(p.outcome_amount, 0) > p.swept_gross \
               AND NOT EXISTS (SELECT 1 FROM sweeps s WHERE s.payment_id = p.payment_id \
                 AND (s.status IN ('pending', 'broadcast') \
                      OR (s.status = 'failed' AND s.updated_at > now() - interval '1 hour'))) \
             ORDER BY p.paid_at NULLS FIRST, p.payment_id LIMIT 5000",
        )
        .bind(network_id)
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| EvmSweepCandidate {
                payment_id: r.get("payment_id"),
                project_id: r.get("project_id"),
                asset_network_id: r.get("asset_network_id"),
                available: r.get("available"),
                paid_at: r.get("paid_at"),
                contract_address: r.get("contract_address"),
                decimals: r.get("decimals"),
                coingecko_id: r.get("coingecko_id"),
                deposit: r.get("deposit"),
                factory: r.get("factory"),
                cold_wallet: r.get("cold_wallet"),
                salt_index: r.get("salt_index"),
            })
            .collect())
    }

    /// Sweep rows of a flush transaction.
    pub async fn evm_sweeps_for_tx(&self, network_id: i64, txid: &str) -> Result<Vec<EvmSweepRow>> {
        let rows = sqlx::query(
            "SELECT s.sweep_id, s.payment_id, s.from_address, an.contract_address, an.decimals FROM sweeps s \
             JOIN asset_network an ON an.asset_network_id = s.asset_network_id \
             WHERE s.network_id = $1 AND s.txid = $2 ORDER BY s.sweep_id",
        )
        .bind(network_id)
        .bind(txid)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| EvmSweepRow {
                sweep_id: r.get("sweep_id"),
                payment_id: r.get("payment_id"),
                from_address: r.get("from_address"),
                contract_address: r.get("contract_address"),
                decimals: r.get("decimals"),
            })
            .collect())
    }

    /// Record the outcome of one sweep row. A confirmed amount adds to the payment's swept total;
    /// a confirmed zero means the address had nothing left, so the credited amount counts as swept.
    pub async fn settle_evm_sweep(&self, sweep_id: i64, status: &str, amount: Option<Decimal>, last_error: Option<&str>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let payment_id: Option<i64> = sqlx::query_scalar(
            "UPDATE sweeps SET status = $2, amount = COALESCE($3, amount), last_error = $4, updated_at = now() \
             WHERE sweep_id = $1 RETURNING payment_id",
        )
        .bind(sweep_id)
        .bind(status)
        .bind(amount)
        .bind(last_error)
        .fetch_one(&mut *tx)
        .await?;
        if let (Some(pid), "confirmed") = (payment_id, status) {
            let amount = amount.unwrap_or(Decimal::ZERO);
            sqlx::query(
                "UPDATE payments SET swept_gross = CASE WHEN $2 > 0 THEN swept_gross + $2 \
                   ELSE GREATEST(swept_gross, COALESCE(outcome_amount, 0)) END, updated_at = now() WHERE payment_id = $1",
            )
            .bind(pid)
            .bind(amount)
            .execute(&mut *tx)
            .await?;
            sqlx::query("UPDATE deposit_addresses SET is_swept = true WHERE payment_id = $1").bind(pid).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// The deposit holds nothing of the payment's asset: count the credited amount as swept.
    pub async fn mark_evm_payment_swept(&self, payment_id: i64) -> Result<()> {
        sqlx::query(
            "UPDATE payments SET swept_gross = GREATEST(swept_gross, COALESCE(outcome_amount, 0)), updated_at = now() \
             WHERE payment_id = $1",
        )
        .bind(payment_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ---- inside the nonce-lock transaction ---------------------------------

    /// Serialize nonce use of one hot address on one chain, until the transaction ends.
    /// The payout sender takes the same lock.
    pub async fn lock_evm_nonce(conn: &mut PgConnection, chain: &str, address: &str) -> Result<()> {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('evm-nonce:' || $1 || ':' || lower($2)))")
            .bind(chain)
            .bind(address)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Insert a broadcast transaction, or mark a queued one broadcast. Returns its id.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_evm_tx(
        conn: &mut PgConnection,
        queued_id: Option<i64>,
        network_id: i64,
        project_id: i64,
        kind: &str,
        factory: &str,
        cold_wallet: &str,
        from_address: &str,
        nonce: i64,
        txid: &str,
        salts: i32,
    ) -> Result<i64> {
        if let Some(id) = queued_id {
            sqlx::query(
                "UPDATE evm_txs SET status = 'broadcast', from_address = $2, nonce = $3, txid = $4, updated_at = now() \
                 WHERE evm_tx_id = $1",
            )
            .bind(id)
            .bind(from_address)
            .bind(nonce)
            .bind(txid)
            .execute(conn)
            .await?;
            return Ok(id);
        }
        Ok(sqlx::query_scalar(
            "INSERT INTO evm_txs (network_id, project_id, kind, factory, cold_wallet, from_address, nonce, txid, salts, status) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'broadcast') RETURNING evm_tx_id",
        )
        .bind(network_id)
        .bind(project_id)
        .bind(kind)
        .bind(factory)
        .bind(cold_wallet)
        .bind(from_address)
        .bind(nonce)
        .bind(txid)
        .bind(salts)
        .fetch_one(conn)
        .await?)
    }

    /// A broadcast sweep row of a flush.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_evm_sweep(
        conn: &mut PgConnection,
        project_id: i64,
        network_id: i64,
        asset_network_id: i64,
        payment_id: Option<i64>,
        from: &str,
        to: &str,
        amount: Decimal,
        status: &str,
        txid: &str,
    ) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "INSERT INTO sweeps (project_id, network_id, asset_network_id, payment_id, from_address, to_address, amount, \
               status, txid) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING sweep_id",
        )
        .bind(project_id)
        .bind(network_id)
        .bind(asset_network_id)
        .bind(payment_id)
        .bind(from)
        .bind(to)
        .bind(amount)
        .bind(status)
        .bind(txid)
        .fetch_one(conn)
        .await?)
    }
}
