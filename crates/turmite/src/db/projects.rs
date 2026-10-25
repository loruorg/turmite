use chrono::{DateTime, Utc};
use sqlx::{Row, postgres::PgRow};

use super::PgStore;
use crate::error::{Error, Result};

#[derive(Clone, Debug)]
pub struct ProjectRow {
    pub project_id: i64,
    pub name: String,
    pub ipn_secret: Vec<u8>,
    pub signature_header: String,
    pub payment_expiry_minutes: i32,
    pub invoice_expiry_minutes: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct MemberRow {
    pub user_id: i64,
    pub email: String,
    pub role: String,
}

const PROJECT_COLS: &str = "project_id, name, ipn_secret, signature_header, payment_expiry_minutes, invoice_expiry_minutes, created_at";

fn project_from_row(r: &PgRow) -> ProjectRow {
    ProjectRow {
        project_id: r.get("project_id"),
        name: r.get("name"),
        ipn_secret: r.get("ipn_secret"),
        signature_header: r.get("signature_header"),
        payment_expiry_minutes: r.get("payment_expiry_minutes"),
        invoice_expiry_minutes: r.get("invoice_expiry_minutes"),
        created_at: r.get("created_at"),
    }
}

impl PgStore {
    /// `ipn_secret_sealed` is the sealed IPN secret.
    pub async fn create_project(&self, name: &str, ipn_secret_sealed: &[u8]) -> Result<i64> {
        Ok(sqlx::query_scalar("INSERT INTO projects (name, ipn_secret) VALUES ($1,$2) RETURNING project_id")
            .bind(name.trim())
            .bind(ipn_secret_sealed)
            .fetch_one(&self.pool)
            .await?)
    }

    pub async fn project(&self, project_id: i64) -> Result<ProjectRow> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {PROJECT_COLS} FROM projects WHERE project_id = $1")))
            .bind(project_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)?;
        Ok(project_from_row(&row))
    }

    pub async fn list_projects(&self) -> Result<Vec<ProjectRow>> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {PROJECT_COLS} FROM projects ORDER BY project_id")))
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(project_from_row).collect())
    }

    /// Projects a user can open: all for admins, else the ones they are a member of.
    pub async fn projects_for_user(&self, user_id: i64, is_admin: bool) -> Result<Vec<ProjectRow>> {
        if is_admin {
            return self.list_projects().await;
        }
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {PROJECT_COLS} FROM projects WHERE project_id IN \
             (SELECT project_id FROM project_members WHERE user_id = $1) ORDER BY project_id"
        )))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(project_from_row).collect())
    }

    pub async fn rename_project(&self, project_id: i64, name: &str) -> Result<()> {
        sqlx::query("UPDATE projects SET name = $1, updated_at = now() WHERE project_id = $2")
            .bind(name.trim())
            .bind(project_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_project_ipn_secret(&self, project_id: i64, sealed: &[u8]) -> Result<()> {
        sqlx::query("UPDATE projects SET ipn_secret = $1, updated_at = now() WHERE project_id = $2")
            .bind(sealed)
            .bind(project_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_project_payment_settings(
        &self,
        project_id: i64,
        signature_header: &str,
        payment_expiry_minutes: i32,
        invoice_expiry_minutes: i32,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE projects SET signature_header = $1, payment_expiry_minutes = $2, invoice_expiry_minutes = $3, \
             updated_at = now() WHERE project_id = $4",
        )
        .bind(signature_header)
        .bind(payment_expiry_minutes)
        .bind(invoice_expiry_minutes)
        .bind(project_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ---- members -----------------------------------------------------------

    /// The project role of a member, if any.
    pub async fn member_role(&self, project_id: i64, user_id: i64) -> Result<Option<String>> {
        Ok(sqlx::query_scalar("SELECT role FROM project_members WHERE project_id = $1 AND user_id = $2")
            .bind(project_id)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn list_members(&self, project_id: i64) -> Result<Vec<MemberRow>> {
        let rows = sqlx::query(
            "SELECT u.user_id, u.email, m.role FROM project_members m JOIN users u ON u.user_id = m.user_id \
             WHERE m.project_id = $1 ORDER BY u.email",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(|r| MemberRow { user_id: r.get("user_id"), email: r.get("email"), role: r.get("role") }).collect())
    }

    pub async fn set_member(&self, project_id: i64, user_id: i64, role: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO project_members (project_id, user_id, role) VALUES ($1,$2,$3) \
             ON CONFLICT (project_id, user_id) DO UPDATE SET role = EXCLUDED.role",
        )
        .bind(project_id)
        .bind(user_id)
        .bind(role)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn remove_member(&self, project_id: i64, user_id: i64) -> Result<()> {
        sqlx::query("DELETE FROM project_members WHERE project_id = $1 AND user_id = $2")
            .bind(project_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
