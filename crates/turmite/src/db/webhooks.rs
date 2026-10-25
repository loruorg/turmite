use chrono::{DateTime, Utc};
use sqlx::{Row, postgres::PgRow};

use super::{Page, PgStore};
use crate::error::Result;

#[derive(Clone, Debug)]
pub struct EndpointRow {
    pub webhook_endpoint_id: i64,
    pub project_id: i64,
    pub url: String,
    pub secret: Vec<u8>,
    pub is_enabled: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct DeliveryRow {
    pub webhook_delivery_id: i64,
    pub delivery_token: String,
    pub webhook_endpoint_id: Option<i64>,
    pub url: String,
    pub event_type: String,
    pub is_final: bool,
    pub status: String,
    pub attempt_count: i32,
    pub last_status_code: Option<i32>,
    pub last_error: Option<String>,
    pub next_retry_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// A claimed delivery with what is needed to sign it.
#[derive(Clone, Debug)]
pub struct DueDelivery {
    pub webhook_delivery_id: i64,
    pub delivery_token: String,
    pub project_id: i64,
    pub url: String,
    pub event_type: String,
    pub is_final: bool,
    pub payload: serde_json::Value,
    pub attempt_count: i32,
    /// Sealed secret of the endpoint, or of the project for IPN callback URLs.
    pub secret: Vec<u8>,
    pub from_endpoint: bool,
    pub signature_header: String,
}

fn endpoint_from_row(r: &PgRow) -> EndpointRow {
    EndpointRow {
        webhook_endpoint_id: r.get("webhook_endpoint_id"),
        project_id: r.get("project_id"),
        url: r.get("url"),
        secret: r.get("secret"),
        is_enabled: r.get("is_enabled"),
        created_at: r.get("created_at"),
    }
}

impl PgStore {
    pub async fn add_webhook_endpoint(&self, project_id: i64, url: &str, secret_sealed: &[u8]) -> Result<i64> {
        Ok(sqlx::query_scalar("INSERT INTO webhook_endpoints (project_id, url, secret) VALUES ($1,$2,$3) RETURNING webhook_endpoint_id")
            .bind(project_id)
            .bind(url)
            .bind(secret_sealed)
            .fetch_one(&self.pool)
            .await?)
    }

    pub async fn list_webhook_endpoints(&self, project_id: i64) -> Result<Vec<EndpointRow>> {
        let rows = sqlx::query(
            "SELECT webhook_endpoint_id, project_id, url, secret, is_enabled, created_at FROM webhook_endpoints \
             WHERE project_id = $1 ORDER BY webhook_endpoint_id",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(endpoint_from_row).collect())
    }

    pub async fn delete_webhook_endpoint(&self, project_id: i64, endpoint_id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM webhook_endpoints WHERE project_id = $1 AND webhook_endpoint_id = $2")
            .bind(project_id)
            .bind(endpoint_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Queue one delivery. Idempotent per (target, event key).
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_delivery(
        &self,
        project_id: i64,
        endpoint_id: Option<i64>,
        url: &str,
        event_type: &str,
        event_key: &str,
        is_final: bool,
        payload: &serde_json::Value,
        delivery_token: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO webhook_deliveries (delivery_token, project_id, webhook_endpoint_id, url, event_type, event_key, \
               is_final, payload) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) \
             ON CONFLICT (project_id, COALESCE(webhook_endpoint_id, 0), url, event_key) DO NOTHING",
        )
        .bind(delivery_token)
        .bind(project_id)
        .bind(endpoint_id)
        .bind(url)
        .bind(event_type)
        .bind(event_key)
        .bind(is_final)
        .bind(payload)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Claim due deliveries and mark them `delivering`. Deliveries stuck in
    /// `delivering` for 10 minutes (a crash) are claimed again.
    pub async fn claim_due_deliveries(&self, limit: i64) -> Result<Vec<DueDelivery>> {
        let rows = sqlx::query(
            "WITH claimed AS ( \
               UPDATE webhook_deliveries SET status = 'delivering', updated_at = now() \
               WHERE webhook_delivery_id IN ( \
                 SELECT webhook_delivery_id FROM webhook_deliveries \
                 WHERE (status IN ('pending','failed') AND next_retry_at <= now()) \
                    OR (status = 'delivering' AND updated_at < now() - interval '10 minutes') \
                 ORDER BY next_retry_at LIMIT $1 FOR UPDATE SKIP LOCKED) \
               RETURNING webhook_delivery_id, delivery_token, project_id, webhook_endpoint_id, url, event_type, \
                 is_final, payload, attempt_count) \
             SELECT c.*, COALESCE(e.secret, p.ipn_secret) AS secret, p.signature_header \
             FROM claimed c JOIN projects p ON p.project_id = c.project_id \
             LEFT JOIN webhook_endpoints e ON e.webhook_endpoint_id = c.webhook_endpoint_id",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| DueDelivery {
                webhook_delivery_id: r.get("webhook_delivery_id"),
                delivery_token: r.get("delivery_token"),
                project_id: r.get("project_id"),
                url: r.get("url"),
                event_type: r.get("event_type"),
                is_final: r.get("is_final"),
                payload: r.get("payload"),
                attempt_count: r.get("attempt_count"),
                secret: r.get("secret"),
                from_endpoint: r.get::<Option<i64>, _>("webhook_endpoint_id").is_some(),
                signature_header: r.get("signature_header"),
            })
            .collect())
    }

    pub async fn mark_delivery_delivered(&self, id: i64, status_code: i32) -> Result<()> {
        sqlx::query(
            "UPDATE webhook_deliveries SET status = 'delivered', last_status_code = $2, last_error = NULL, \
               attempt_count = attempt_count + 1, delivered_at = now(), updated_at = now() WHERE webhook_delivery_id = $1",
        )
        .bind(id)
        .bind(status_code)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record a failed attempt. `retry_in = None` marks the delivery dead.
    pub async fn mark_delivery_failed(&self, id: i64, status_code: Option<i32>, error: &str, retry_in_secs: Option<i64>) -> Result<()> {
        sqlx::query(
            "UPDATE webhook_deliveries SET status = CASE WHEN $4::bigint IS NULL THEN 'dead' ELSE 'failed' END, \
               last_status_code = $2, last_error = $3, attempt_count = attempt_count + 1, \
               next_retry_at = now() + make_interval(secs => COALESCE($4, 0)), updated_at = now() \
             WHERE webhook_delivery_id = $1",
        )
        .bind(id)
        .bind(status_code)
        .bind(error.chars().take(500).collect::<String>())
        .bind(retry_in_secs)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_deliveries(&self, project_id: i64, page: Page) -> Result<Vec<DeliveryRow>> {
        let rows = sqlx::query(
            "SELECT webhook_delivery_id, delivery_token, webhook_endpoint_id, url, event_type, is_final, status, \
               attempt_count, last_status_code, last_error, next_retry_at, delivered_at, created_at \
             FROM webhook_deliveries WHERE project_id = $1 ORDER BY webhook_delivery_id DESC LIMIT $2 OFFSET $3",
        )
        .bind(project_id)
        .bind(page.limit)
        .bind(page.offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| DeliveryRow {
                webhook_delivery_id: r.get("webhook_delivery_id"),
                delivery_token: r.get("delivery_token"),
                webhook_endpoint_id: r.get("webhook_endpoint_id"),
                url: r.get("url"),
                event_type: r.get("event_type"),
                is_final: r.get("is_final"),
                status: r.get("status"),
                attempt_count: r.get("attempt_count"),
                last_status_code: r.get("last_status_code"),
                last_error: r.get("last_error"),
                next_retry_at: r.get("next_retry_at"),
                delivered_at: r.get("delivered_at"),
                created_at: r.get("created_at"),
            })
            .collect())
    }

    /// Schedule one delivery to run now (manual retry, dead ones included).
    pub async fn retry_delivery_now(&self, project_id: i64, delivery_id: i64) -> Result<bool> {
        let res = sqlx::query(
            "UPDATE webhook_deliveries SET status = 'pending', next_retry_at = now(), updated_at = now() \
             WHERE project_id = $1 AND webhook_delivery_id = $2 AND status <> 'delivering'",
        )
        .bind(project_id)
        .bind(delivery_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() == 1)
    }
}
