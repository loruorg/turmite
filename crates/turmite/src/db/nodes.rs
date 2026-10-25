use chrono::{DateTime, Utc};
use sqlx::Row;

use super::PgStore;
use crate::error::Result;

#[derive(Clone, Debug)]
pub struct NodeRow {
    pub node_id: i64,
    pub network_id: i64,
    pub label: String,
    pub url_sealed: Vec<u8>,
    pub url_display: String,
    pub priority: i32,
    pub is_enabled: bool,
    pub last_ok_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub last_error_at: Option<DateTime<Utc>>,
    pub last_height: Option<i64>,
}

impl PgStore {
    pub async fn add_node(&self, network_id: i64, label: &str, url_sealed: &[u8], url_display: &str, priority: i32) -> Result<i64> {
        let id = sqlx::query_scalar(
            "INSERT INTO nodes (network_id, label, url_sealed, url_display, priority) VALUES ($1,$2,$3,$4,$5) \
             RETURNING node_id",
        )
        .bind(network_id)
        .bind(label)
        .bind(url_sealed)
        .bind(url_display)
        .bind(priority)
        .fetch_one(&self.pool)
        .await?;
        self.bump_nodes_version().await?;
        Ok(id)
    }

    pub async fn list_nodes(&self, network_id: Option<i64>) -> Result<Vec<NodeRow>> {
        let rows = sqlx::query(
            "SELECT node_id, network_id, label, url_sealed, url_display, priority, is_enabled, last_ok_at, last_error, \
               last_error_at, last_height FROM nodes WHERE ($1::bigint IS NULL OR network_id = $1) \
             ORDER BY network_id, priority, node_id",
        )
        .bind(network_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| NodeRow {
                node_id: r.get("node_id"),
                network_id: r.get("network_id"),
                label: r.get("label"),
                url_sealed: r.get("url_sealed"),
                url_display: r.get("url_display"),
                priority: r.get("priority"),
                is_enabled: r.get("is_enabled"),
                last_ok_at: r.get("last_ok_at"),
                last_error: r.get("last_error"),
                last_error_at: r.get("last_error_at"),
                last_height: r.get("last_height"),
            })
            .collect())
    }

    pub async fn delete_node(&self, node_id: i64) -> Result<()> {
        sqlx::query("DELETE FROM nodes WHERE node_id = $1").bind(node_id).execute(&self.pool).await?;
        self.bump_nodes_version().await
    }

    pub async fn set_node_enabled(&self, node_id: i64, enabled: bool) -> Result<()> {
        sqlx::query("UPDATE nodes SET is_enabled = $1 WHERE node_id = $2").bind(enabled).bind(node_id).execute(&self.pool).await?;
        self.bump_nodes_version().await
    }

    pub async fn record_node_ok(&self, node_id: i64, height: Option<i64>) -> Result<()> {
        sqlx::query("UPDATE nodes SET last_ok_at = now(), last_height = COALESCE($2, last_height) WHERE node_id = $1")
            .bind(node_id)
            .bind(height)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn record_node_error(&self, node_id: i64, error: &str) -> Result<()> {
        sqlx::query("UPDATE nodes SET last_error = $2, last_error_at = now() WHERE node_id = $1")
            .bind(node_id)
            .bind(error.chars().take(500).collect::<String>())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Driver registries rebuild when this number changes.
    pub async fn nodes_version(&self) -> Result<i64> {
        Ok(self.setting::<i64>("nodes_version").await?.unwrap_or(0))
    }

    pub async fn bump_nodes_version(&self) -> Result<()> {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES ('nodes_version', '1') ON CONFLICT (key) \
             DO UPDATE SET value = to_jsonb(COALESCE((settings.value)::text::bigint, 0) + 1), updated_at = now()",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
