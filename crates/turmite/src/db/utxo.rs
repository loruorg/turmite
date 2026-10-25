//! Watch-only wallet data (Bitcoin and Monero).

use sqlx::Row;

use super::PgStore;
use crate::error::Result;

impl PgStore {
    /// Store the first receive addresses of a project's Bitcoin account key.
    pub async fn set_wallet_check_addresses(&self, project_id: i64, network_id: i64, addresses: &[String]) -> Result<()> {
        sqlx::query("UPDATE project_wallets SET check_addresses = $3, updated_at = now() WHERE project_id = $1 AND network_id = $2")
            .bind(project_id)
            .bind(network_id)
            .bind(addresses)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// (network_id, first receive addresses) of a project's wallets that have them.
    pub async fn wallet_check_addresses(&self, project_id: i64) -> Result<Vec<(i64, Vec<String>)>> {
        let rows =
            sqlx::query("SELECT network_id, check_addresses FROM project_wallets WHERE project_id = $1 AND check_addresses IS NOT NULL")
                .bind(project_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.iter().map(|r| (r.get("network_id"), r.get("check_addresses"))).collect())
    }

    /// Network ids where a project has a Monero view key saved.
    pub async fn wallets_with_view_key(&self, project_id: i64) -> Result<Vec<i64>> {
        Ok(sqlx::query_scalar("SELECT network_id FROM project_wallets WHERE project_id = $1 AND view_key IS NOT NULL")
            .bind(project_id)
            .fetch_all(&self.pool)
            .await?)
    }
}
