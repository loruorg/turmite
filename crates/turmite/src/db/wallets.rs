use sqlx::{Row, postgres::PgRow};

use super::PgStore;
use crate::{domain::ProjectWallet, error::Result};

const WALLET_COLS: &str = "project_wallet_id, project_id, network_id, address, address_extra_id, xpub, restore_height, \
    factory_address, is_enabled, created_at, updated_at";

fn wallet_from_row(r: &PgRow) -> ProjectWallet {
    ProjectWallet {
        project_wallet_id: r.get("project_wallet_id"),
        project_id: r.get("project_id"),
        network_id: r.get("network_id"),
        address: r.get("address"),
        address_extra_id: r.get("address_extra_id"),
        xpub: r.get("xpub"),
        restore_height: r.get("restore_height"),
        factory_address: r.get("factory_address"),
        is_enabled: r.get("is_enabled"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

impl PgStore {
    pub async fn project_wallets(&self, project_id: i64) -> Result<Vec<ProjectWallet>> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {WALLET_COLS} FROM project_wallets WHERE project_id = $1 ORDER BY network_id"
        )))
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(wallet_from_row).collect())
    }

    pub async fn project_wallet(&self, project_id: i64, network_id: i64) -> Result<Option<ProjectWallet>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {WALLET_COLS} FROM project_wallets WHERE project_id = $1 AND network_id = $2"
        )))
        .bind(project_id)
        .bind(network_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.as_ref().map(wallet_from_row))
    }

    /// Sealed Monero view key of a project wallet.
    pub async fn project_wallet_view_key(&self, project_id: i64, network_id: i64) -> Result<Option<Vec<u8>>> {
        Ok(sqlx::query_scalar("SELECT view_key FROM project_wallets WHERE project_id = $1 AND network_id = $2")
            .bind(project_id)
            .bind(network_id)
            .fetch_optional(&self.pool)
            .await?
            .flatten())
    }

    /// Set the watch-only and contract fields of a project wallet. `None` leaves a field as it is.
    #[allow(clippy::too_many_arguments)]
    pub async fn set_project_wallet_keys(
        &self,
        project_id: i64,
        network_id: i64,
        xpub: Option<&str>,
        view_key_sealed: Option<&[u8]>,
        restore_height: Option<i64>,
        factory_address: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO project_wallets (project_id, network_id, xpub, view_key, restore_height, factory_address) \
             VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (project_id, network_id) DO UPDATE SET \
               xpub = COALESCE(EXCLUDED.xpub, project_wallets.xpub), \
               view_key = COALESCE(EXCLUDED.view_key, project_wallets.view_key), \
               restore_height = COALESCE(EXCLUDED.restore_height, project_wallets.restore_height), \
               factory_address = COALESCE(EXCLUDED.factory_address, project_wallets.factory_address), \
               updated_at = now()",
        )
        .bind(project_id)
        .bind(network_id)
        .bind(xpub)
        .bind(view_key_sealed)
        .bind(restore_height)
        .bind(factory_address)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Set the settlement address of a project on a network.
    pub async fn upsert_project_wallet_address(
        &self,
        project_id: i64,
        network_id: i64,
        address: &str,
        address_extra_id: Option<&str>,
    ) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "INSERT INTO project_wallets (project_id, network_id, address, address_extra_id) VALUES ($1,$2,$3,$4) \
             ON CONFLICT (project_id, network_id) DO UPDATE SET address = EXCLUDED.address, \
               address_extra_id = EXCLUDED.address_extra_id, is_enabled = true, updated_at = now() \
             RETURNING project_wallet_id",
        )
        .bind(project_id)
        .bind(network_id)
        .bind(address.trim())
        .bind(address_extra_id.map(str::trim).filter(|s| !s.is_empty()))
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn set_project_wallet_enabled(&self, project_id: i64, network_id: i64, enabled: bool) -> Result<()> {
        sqlx::query("UPDATE project_wallets SET is_enabled = $3, updated_at = now() WHERE project_id = $1 AND network_id = $2")
            .bind(project_id)
            .bind(network_id)
            .bind(enabled)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Asset networks a project can take: the network and the asset are enabled and the
    /// project has an enabled wallet on the network.
    pub async fn enabled_asset_network_ids(&self, project_id: i64) -> Result<Vec<i64>> {
        Ok(sqlx::query_scalar(
            "SELECT an.asset_network_id FROM asset_network an \
             JOIN network n ON n.network_id = an.network_id \
             JOIN project_wallets w ON w.network_id = an.network_id AND w.project_id = $1 \
             WHERE an.is_enabled AND n.is_enabled AND w.is_enabled \
             ORDER BY n.network_id, an.asset_network_id",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Asset networks that any project can take (used when no API key is sent).
    pub async fn enabled_asset_network_ids_any(&self) -> Result<Vec<i64>> {
        Ok(sqlx::query_scalar(
            "SELECT DISTINCT an.asset_network_id FROM asset_network an \
             JOIN network n ON n.network_id = an.network_id \
             JOIN project_wallets w ON w.network_id = an.network_id \
             WHERE an.is_enabled AND n.is_enabled AND w.is_enabled ORDER BY an.asset_network_id",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    // ---- deposit addresses -------------------------------------------------

    /// Allocate the next install-wide derivation index for a network (starts at 1).
    pub async fn next_network_index(&self, network_id: i64) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "INSERT INTO network_address_counter (network_id, next_index) VALUES ($1, 2) \
             ON CONFLICT (network_id) DO UPDATE SET next_index = network_address_counter.next_index + 1 \
             RETURNING next_index - 1",
        )
        .bind(network_id)
        .fetch_one(&self.pool)
        .await?)
    }

    /// Allocate the next derivation index of a project wallet (starts at 0).
    pub async fn next_wallet_index(&self, project_id: i64, network_id: i64) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "UPDATE project_wallets SET next_index = next_index + 1 WHERE project_id = $1 AND network_id = $2 \
             RETURNING next_index - 1",
        )
        .bind(project_id)
        .bind(network_id)
        .fetch_one(&self.pool)
        .await?)
    }

    /// Store a derived address. `project_id = None` puts it in the shared pool.
    /// After a database restore: move every address counter `by` indexes on, and delete the
    /// free pool addresses. New payments then do not get the addresses that Turmite handed
    /// out after the dump. Returns the number of pool addresses deleted.
    pub async fn raise_address_counters(&self, by: i64) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        for table in ["network_address_counter", "family_address_counter", "project_wallets"] {
            sqlx::query(sqlx::AssertSqlSafe(format!("UPDATE {table} SET next_index = next_index + $1"))).bind(by).execute(&mut *tx).await?;
        }
        let deleted = sqlx::query("DELETE FROM deposit_addresses WHERE project_id IS NULL AND payment_id IS NULL")
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(deleted)
    }

    pub async fn insert_deposit_address(
        &self,
        network_id: i64,
        project_id: Option<i64>,
        derivation_index: i64,
        address: &str,
        payin_extra_id: Option<&str>,
    ) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "INSERT INTO deposit_addresses (network_id, project_id, derivation_index, address, payin_extra_id) \
             VALUES ($1,$2,$3,$4,$5) RETURNING deposit_address_id",
        )
        .bind(network_id)
        .bind(project_id)
        .bind(derivation_index)
        .bind(address)
        .bind(payin_extra_id)
        .fetch_one(&self.pool)
        .await?)
    }

    /// Unused addresses in the shared pool of a network.
    pub async fn pool_size(&self, network_id: i64) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT COUNT(*) FROM deposit_addresses WHERE network_id = $1 AND project_id IS NULL AND payment_id IS NULL")
            .bind(network_id)
            .fetch_one(&self.pool)
            .await?)
    }

    /// Take one address from the shared pool for a project. Returns (id, address, extra id).
    pub async fn take_pool_address(&self, network_id: i64, project_id: i64) -> Result<Option<(i64, String, Option<String>)>> {
        let row = sqlx::query(
            "UPDATE deposit_addresses SET project_id = $2, reserved_at = now() WHERE deposit_address_id = ( \
               SELECT deposit_address_id FROM deposit_addresses \
               WHERE network_id = $1 AND project_id IS NULL AND payment_id IS NULL \
               ORDER BY deposit_address_id LIMIT 1 FOR UPDATE SKIP LOCKED) \
             RETURNING deposit_address_id, address, payin_extra_id",
        )
        .bind(network_id)
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| (r.get("deposit_address_id"), r.get("address"), r.get("payin_extra_id"))))
    }

    /// Give back a reserved address that no payment took: to the shared pool, or delete it.
    pub async fn release_deposit_address(&self, deposit_address_id: i64, to_pool: bool) -> Result<()> {
        let sql = if to_pool {
            "UPDATE deposit_addresses SET project_id = NULL, reserved_at = NULL WHERE deposit_address_id = $1 AND payment_id IS NULL"
        } else {
            "DELETE FROM deposit_addresses WHERE deposit_address_id = $1 AND payment_id IS NULL"
        };
        sqlx::query(sql).bind(deposit_address_id).execute(&self.pool).await?;
        Ok(())
    }

    /// (derivation_index, address) of a payment's deposit address.
    pub async fn deposit_address_for_payment(&self, payment_id: i64) -> Result<Option<(i64, String)>> {
        let row = sqlx::query("SELECT derivation_index, address FROM deposit_addresses WHERE payment_id = $1")
            .bind(payment_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| (r.get("derivation_index"), r.get("address"))))
    }

    pub async fn mark_deposit_swept(&self, payment_id: i64) -> Result<()> {
        sqlx::query("UPDATE deposit_addresses SET is_swept = true WHERE payment_id = $1").bind(payment_id).execute(&self.pool).await?;
        Ok(())
    }
}
