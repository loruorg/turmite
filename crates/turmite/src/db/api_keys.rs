use chrono::{DateTime, Utc};
use sqlx::Row;

use super::PgStore;
use crate::{
    crypto::sha256_hex,
    error::{Error, Result},
    ids,
};

#[derive(Clone, Debug)]
pub struct ApiKeyRow {
    pub api_key_id: i64,
    pub label: String,
    pub token_last4: String,
    pub created_by_email: Option<String>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl PgStore {
    /// Create a key. Returns the full token; only its hash is stored.
    pub async fn create_api_key(&self, project_id: i64, label: &str, created_by: Option<i64>) -> Result<(String, i64)> {
        let (token, last4) = ids::new_api_token("tm_");
        let id = sqlx::query_scalar(
            "INSERT INTO api_keys (project_id, label, token_hash, token_last4, created_by) VALUES ($1,$2,$3,$4,$5) \
             RETURNING api_key_id",
        )
        .bind(project_id)
        .bind(label.trim())
        .bind(sha256_hex(token.as_bytes()))
        .bind(&last4)
        .bind(created_by)
        .fetch_one(&self.pool)
        .await?;
        Ok((token, id))
    }

    /// Resolve a token to its project and mark it used.
    pub async fn verify_api_key(&self, token: &str) -> Result<(i64, i64)> {
        let row = sqlx::query(
            "UPDATE api_keys SET last_used_at = now() WHERE token_hash = $1 AND revoked_at IS NULL \
             RETURNING project_id, api_key_id",
        )
        .bind(sha256_hex(token.trim().as_bytes()))
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| (r.get("project_id"), r.get("api_key_id"))).ok_or_else(|| Error::Unauthorized("Invalid API key.".into()))
    }

    pub async fn list_api_keys(&self, project_id: i64) -> Result<Vec<ApiKeyRow>> {
        let rows = sqlx::query(
            "SELECT k.api_key_id, k.label, k.token_last4, u.email, k.last_used_at, k.revoked_at, k.created_at \
             FROM api_keys k LEFT JOIN users u ON u.user_id = k.created_by \
             WHERE k.project_id = $1 ORDER BY k.revoked_at NULLS FIRST, k.created_at DESC",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| ApiKeyRow {
                api_key_id: r.get("api_key_id"),
                label: r.get("label"),
                token_last4: r.get("token_last4"),
                created_by_email: r.get("email"),
                last_used_at: r.get("last_used_at"),
                revoked_at: r.get("revoked_at"),
                created_at: r.get("created_at"),
            })
            .collect())
    }

    pub async fn revoke_api_key(&self, project_id: i64, api_key_id: i64) -> Result<bool> {
        let res = sqlx::query("UPDATE api_keys SET revoked_at = now() WHERE project_id = $1 AND api_key_id = $2 AND revoked_at IS NULL")
            .bind(project_id)
            .bind(api_key_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() == 1)
    }
}
