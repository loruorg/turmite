use serde::{Serialize, de::DeserializeOwned};

use super::PgStore;
use crate::error::Result;

impl PgStore {
    pub async fn setting<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let value: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT value FROM settings WHERE key = $1").bind(key).fetch_optional(&self.pool).await?;
        Ok(value.and_then(|v| serde_json::from_value(v).ok()))
    }

    pub async fn set_setting<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES ($1,$2) \
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
        )
        .bind(key)
        .bind(serde_json::to_value(value).unwrap_or_default())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Insert only if absent. Returns the stored value (the existing one on conflict).
    pub async fn init_setting<T: Serialize + DeserializeOwned>(&self, key: &str, value: &T) -> Result<Option<T>> {
        sqlx::query("INSERT INTO settings (key, value) VALUES ($1,$2) ON CONFLICT (key) DO NOTHING")
            .bind(key)
            .bind(serde_json::to_value(value).unwrap_or_default())
            .execute(&self.pool)
            .await?;
        self.setting(key).await
    }
}
