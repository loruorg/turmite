use chrono::{DateTime, Utc};
use sqlx::{Row, postgres::PgRow};

use super::PgStore;
use crate::error::{Error, Result};

#[derive(Clone, Debug)]
pub struct UserRow {
    pub user_id: i64,
    pub email: String,
    pub password_hash: String,
    pub role: String,
    pub totp_secret: Option<Vec<u8>>,
    pub totp_enabled: bool,
    pub totp_last_step: i64,
    pub disabled_at: Option<DateTime<Utc>>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl UserRow {
    pub fn is_admin(&self) -> bool {
        matches!(self.role.as_str(), "root" | "admin")
    }
    pub fn is_root(&self) -> bool {
        self.role == "root"
    }
}

/// The user behind a session cookie.
#[derive(Clone, Debug)]
pub struct SessionUser {
    pub user: UserRow,
    pub session_hash: String,
}

const USER_COLS: &str = "user_id, email, password_hash, role, totp_secret, totp_enabled, totp_last_step, \
    disabled_at, last_login_at, created_at";

fn user_from_row(r: &PgRow) -> UserRow {
    UserRow {
        user_id: r.get("user_id"),
        email: r.get("email"),
        password_hash: r.get("password_hash"),
        role: r.get("role"),
        totp_secret: r.get("totp_secret"),
        totp_enabled: r.get("totp_enabled"),
        totp_last_step: r.get("totp_last_step"),
        disabled_at: r.get("disabled_at"),
        last_login_at: r.get("last_login_at"),
        created_at: r.get("created_at"),
    }
}

impl PgStore {
    pub async fn user_count(&self) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(&self.pool).await?)
    }

    pub async fn create_user(&self, email: &str, password_hash: &str, role: &str) -> Result<i64> {
        let res = sqlx::query_scalar("INSERT INTO users (email, password_hash, role) VALUES ($1,$2,$3) RETURNING user_id")
            .bind(email.trim())
            .bind(password_hash)
            .bind(role)
            .fetch_one(&self.pool)
            .await;
        match res {
            Ok(id) => Ok(id),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                Err(Error::Conflict("A user with this email or role already exists.".into()))
            }
            Err(e) => Err(e.into()),
        }
    }

    pub async fn user_by_email(&self, email: &str) -> Result<Option<UserRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {USER_COLS} FROM users WHERE LOWER(email) = LOWER($1)")))
            .bind(email.trim())
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(user_from_row))
    }

    pub async fn user(&self, user_id: i64) -> Result<UserRow> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {USER_COLS} FROM users WHERE user_id = $1")))
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)?;
        Ok(user_from_row(&row))
    }

    pub async fn list_users(&self) -> Result<Vec<UserRow>> {
        let rows =
            sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {USER_COLS} FROM users ORDER BY user_id"))).fetch_all(&self.pool).await?;
        Ok(rows.iter().map(user_from_row).collect())
    }

    pub async fn set_user_password(&self, user_id: i64, password_hash: &str) -> Result<()> {
        sqlx::query("UPDATE users SET password_hash = $1 WHERE user_id = $2").bind(password_hash).bind(user_id).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn set_user_role(&self, user_id: i64, role: &str) -> Result<()> {
        sqlx::query("UPDATE users SET role = $1 WHERE user_id = $2 AND role <> 'root'")
            .bind(role)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_user_disabled(&self, user_id: i64, disabled: bool) -> Result<()> {
        sqlx::query("UPDATE users SET disabled_at = CASE WHEN $1 THEN now() ELSE NULL END WHERE user_id = $2 AND role <> 'root'")
            .bind(disabled)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        if disabled {
            self.delete_user_sessions(user_id).await?;
        }
        Ok(())
    }

    /// Store a sealed TOTP secret that is not active yet.
    pub async fn set_pending_totp(&self, user_id: i64, sealed: &[u8]) -> Result<()> {
        sqlx::query("UPDATE users SET totp_secret = $1, totp_enabled = false, totp_last_step = 0 WHERE user_id = $2")
            .bind(sealed)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn enable_totp(&self, user_id: i64, step: i64) -> Result<()> {
        sqlx::query("UPDATE users SET totp_enabled = true, totp_last_step = $1 WHERE user_id = $2")
            .bind(step)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn disable_totp(&self, user_id: i64) -> Result<()> {
        sqlx::query("UPDATE users SET totp_enabled = false, totp_secret = NULL, totp_last_step = 0 WHERE user_id = $1")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Record a used TOTP step. Fails if a newer or equal step was already used.
    pub async fn consume_totp_step(&self, user_id: i64, step: i64) -> Result<bool> {
        let res = sqlx::query("UPDATE users SET totp_last_step = $1 WHERE user_id = $2 AND totp_last_step < $1")
            .bind(step)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() == 1)
    }

    pub async fn touch_login(&self, user_id: i64) -> Result<()> {
        sqlx::query("UPDATE users SET last_login_at = now() WHERE user_id = $1").bind(user_id).execute(&self.pool).await?;
        Ok(())
    }

    // ---- sessions ----------------------------------------------------------

    pub async fn create_session(
        &self,
        session_hash: &str,
        user_id: i64,
        ip: Option<&str>,
        user_agent: Option<&str>,
        expires_at: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query("INSERT INTO sessions (session_hash, user_id, ip, user_agent, expires_at) VALUES ($1,$2,$3,$4,$5)")
            .bind(session_hash)
            .bind(user_id)
            .bind(ip)
            .bind(user_agent.map(|s| s.chars().take(300).collect::<String>()))
            .bind(expires_at)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The active user for a session, extending the session's idle expiry.
    pub async fn session_user(&self, session_hash: &str, idle_extend_to: DateTime<Utc>) -> Result<Option<SessionUser>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "WITH s AS (UPDATE sessions SET last_seen_at = now(), expires_at = LEAST(GREATEST(expires_at, $2), created_at + interval '7 days') \
               WHERE session_hash = $1 AND expires_at > now() RETURNING user_id) \
             SELECT {USER_COLS} FROM users WHERE user_id = (SELECT user_id FROM s) AND disabled_at IS NULL"
        )))
        .bind(session_hash)
        .bind(idle_extend_to)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| SessionUser { user: user_from_row(&r), session_hash: session_hash.to_string() }))
    }

    pub async fn delete_session(&self, session_hash: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE session_hash = $1").bind(session_hash).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn delete_user_sessions(&self, user_id: i64) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(user_id).execute(&self.pool).await?;
        Ok(())
    }

    /// End every session of the user except `keep_hash`.
    pub async fn delete_other_sessions(&self, user_id: i64, keep_hash: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND session_hash <> $2")
            .bind(user_id)
            .bind(keep_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn delete_expired_sessions(&self) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE expires_at < now()").execute(&self.pool).await?;
        Ok(())
    }
}
