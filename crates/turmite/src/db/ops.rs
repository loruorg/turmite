//! Node health and the token list of the Networks page.

use super::PgStore;
use crate::error::Result;

impl PgStore {
    /// Save the best tip height of a network. `last_block_at` is the time the height last
    /// went up, so its age shows a chain or node that stopped. A much lower height is a
    /// reset chain (local test networks) and is saved too.
    pub async fn advance_last_block(&self, network_id: i64, height: i64) -> Result<()> {
        sqlx::query(
            "UPDATE network SET last_block = $1, last_block_at = now() \
             WHERE network_id = $2 AND (last_block IS NULL OR $1 > last_block OR $1 < last_block - 1000)",
        )
        .bind(height)
        .bind(network_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// (symbol, name) of every asset, for the token form.
    pub async fn list_asset_symbols(&self) -> Result<Vec<(String, String)>> {
        Ok(sqlx::query_as("SELECT symbol, name FROM asset ORDER BY symbol").fetch_all(&self.pool).await?)
    }

    /// Email addresses of the root and admin accounts that are not disabled.
    pub async fn admin_emails(&self) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar("SELECT email FROM users WHERE role IN ('root', 'admin') AND disabled_at IS NULL ORDER BY user_id")
            .fetch_all(&self.pool)
            .await?)
    }
}
