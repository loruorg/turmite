use chrono::{DateTime, Utc};
use sqlx::Row;

use super::{Page, PgStore};
use crate::error::Result;

#[derive(Clone, Debug)]
pub struct AuditRow {
    pub audit_id: i64,
    pub user_email: Option<String>,
    pub project_id: Option<i64>,
    pub project_name: Option<String>,
    pub actor: String,
    pub action: String,
    pub entity: String,
    pub entity_id: Option<String>,
    pub data: serde_json::Value,
    pub ip: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl PgStore {
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_audit(
        &self,
        user_id: Option<i64>,
        project_id: Option<i64>,
        actor: &str,
        action: &str,
        entity: &str,
        entity_id: Option<&str>,
        data: &serde_json::Value,
        ip: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO audit_log (user_id, project_id, actor, action, entity, entity_id, data, ip) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(user_id)
        .bind(project_id)
        .bind(actor)
        .bind(action)
        .bind(entity)
        .bind(entity_id)
        .bind(data)
        .bind(ip)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Newest first. `project_id = None` lists every entry.
    pub async fn list_audit(&self, project_id: Option<i64>, page: Page) -> Result<Vec<AuditRow>> {
        let rows = sqlx::query(
            "SELECT a.audit_id, u.email, a.project_id, p.name AS project_name, a.actor, a.action, a.entity, \
               a.entity_id, a.data, a.ip, a.created_at \
             FROM audit_log a LEFT JOIN users u ON u.user_id = a.user_id \
             LEFT JOIN projects p ON p.project_id = a.project_id \
             WHERE ($1::bigint IS NULL OR a.project_id = $1) \
             ORDER BY a.audit_id DESC LIMIT $2 OFFSET $3",
        )
        .bind(project_id)
        .bind(page.limit)
        .bind(page.offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| AuditRow {
                audit_id: r.get("audit_id"),
                user_email: r.get("email"),
                project_id: r.get("project_id"),
                project_name: r.get("project_name"),
                actor: r.get("actor"),
                action: r.get("action"),
                entity: r.get("entity"),
                entity_id: r.get("entity_id"),
                data: r.get("data"),
                ip: r.get("ip"),
                created_at: r.get("created_at"),
            })
            .collect())
    }
}
