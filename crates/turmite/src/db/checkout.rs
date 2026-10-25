//! Checkout branding, checkout domains and exchange trades.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{Row, postgres::PgRow};

use super::PgStore;
use crate::error::{Error, Result};

/// How a project's checkout looks.
#[derive(Clone, Debug, Default)]
pub struct BrandingRow {
    pub project_id: i64,
    pub name: String,
    pub logo_hash: Option<String>,
    pub logo_url: Option<String>,
    pub accent_color: Option<String>,
    pub theme: String,
    pub message: Option<String>,
    pub support_email: Option<String>,
    pub support_url: Option<String>,
    pub return_url: Option<String>,
    pub return_hosts: Vec<String>,
    pub checkout_domain: Option<String>,
}

/// Branding fields edited on the settings page.
#[derive(Clone, Debug, Default)]
pub struct BrandingUpdate {
    pub accent_color: Option<String>,
    pub theme: String,
    pub message: Option<String>,
    pub support_email: Option<String>,
    pub support_url: Option<String>,
    pub return_url: Option<String>,
    pub return_hosts: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ExchangeSettings {
    pub enabled: bool,
    pub key_sealed: Option<Vec<u8>>,
    pub markup: Decimal,
    pub min_kyc: String,
    pub coins: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ExchangeCoin {
    pub ticker: String,
    pub network: String,
    pub name: String,
    pub memo: bool,
    pub minimum: Option<Decimal>,
    pub maximum: Option<Decimal>,
}

#[derive(Clone, Debug)]
pub struct TradeRow {
    pub exchange_trade_id: i64,
    pub payment_id: i64,
    pub project_id: i64,
    pub token: String,
    pub hook_secret: String,
    pub webhook_key: String,
    pub trade_id: String,
    pub provider: String,
    pub kyc_rating: Option<String>,
    pub coin_from: String,
    pub ticker_from: String,
    pub network_from: String,
    pub ticker_to: String,
    pub network_to: String,
    pub amount_from: Decimal,
    pub amount_to: Decimal,
    pub address_provider: String,
    pub address_provider_memo: Option<String>,
    pub refund_address: Option<String>,
    pub refund_memo: Option<String>,
    pub status: String,
    pub hashout: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub status_changed_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// Fields of a new trade.
#[derive(Clone, Debug)]
pub struct NewTrade<'a> {
    pub payment_id: i64,
    pub project_id: i64,
    pub token: &'a str,
    pub hook_secret: &'a str,
    pub webhook_key: &'a str,
    pub trade_id: &'a str,
    pub provider: &'a str,
    pub kyc_rating: Option<&'a str>,
    pub coin_from: &'a str,
    pub ticker_from: &'a str,
    pub network_from: &'a str,
    pub ticker_to: &'a str,
    pub network_to: &'a str,
    pub amount_from: Decimal,
    pub amount_to: Decimal,
    pub address_provider: &'a str,
    pub address_provider_memo: Option<&'a str>,
    pub refund_address: Option<&'a str>,
    pub refund_memo: Option<&'a str>,
    pub status: &'a str,
    pub expires_at: Option<DateTime<Utc>>,
}

const BRANDING_COLS: &str = "project_id, name, logo_hash, logo_url, accent_color, checkout_theme, checkout_message, \
    support_email, support_url, return_url, return_hosts, checkout_domain";

const TRADE_COLS: &str = "exchange_trade_id, payment_id, project_id, token, hook_secret, webhook_key, trade_id, provider, \
    kyc_rating, coin_from, ticker_from, network_from, ticker_to, network_to, amount_from, amount_to, address_provider, \
    address_provider_memo, refund_address, refund_memo, status, hashout, expires_at, status_changed_at, created_at";

fn branding_from_row(r: &PgRow) -> BrandingRow {
    BrandingRow {
        project_id: r.get("project_id"),
        name: r.get("name"),
        logo_hash: r.get("logo_hash"),
        logo_url: r.get("logo_url"),
        accent_color: r.get("accent_color"),
        theme: r.get("checkout_theme"),
        message: r.get("checkout_message"),
        support_email: r.get("support_email"),
        support_url: r.get("support_url"),
        return_url: r.get("return_url"),
        return_hosts: r.get("return_hosts"),
        checkout_domain: r.get("checkout_domain"),
    }
}

fn trade_from_row(r: &PgRow) -> TradeRow {
    TradeRow {
        exchange_trade_id: r.get("exchange_trade_id"),
        payment_id: r.get("payment_id"),
        project_id: r.get("project_id"),
        token: r.get("token"),
        hook_secret: r.get("hook_secret"),
        webhook_key: r.get("webhook_key"),
        trade_id: r.get("trade_id"),
        provider: r.get("provider"),
        kyc_rating: r.get("kyc_rating"),
        coin_from: r.get("coin_from"),
        ticker_from: r.get("ticker_from"),
        network_from: r.get("network_from"),
        ticker_to: r.get("ticker_to"),
        network_to: r.get("network_to"),
        amount_from: r.get("amount_from"),
        amount_to: r.get("amount_to"),
        address_provider: r.get("address_provider"),
        address_provider_memo: r.get("address_provider_memo"),
        refund_address: r.get("refund_address"),
        refund_memo: r.get("refund_memo"),
        status: r.get("status"),
        hashout: r.get("hashout"),
        expires_at: r.get("expires_at"),
        status_changed_at: r.get("status_changed_at"),
        created_at: r.get("created_at"),
    }
}

fn coin_from_row(r: &PgRow) -> ExchangeCoin {
    ExchangeCoin {
        ticker: r.get("ticker"),
        network: r.get("network"),
        name: r.get("name"),
        memo: r.get("memo"),
        minimum: r.get("minimum"),
        maximum: r.get("maximum"),
    }
}

impl PgStore {
    // ---- branding ----------------------------------------------------------

    pub async fn branding(&self, project_id: i64) -> Result<BrandingRow> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {BRANDING_COLS} FROM projects WHERE project_id = $1")))
            .bind(project_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)?;
        Ok(branding_from_row(&row))
    }

    pub async fn save_branding(&self, project_id: i64, b: &BrandingUpdate) -> Result<()> {
        sqlx::query(
            "UPDATE projects SET accent_color = $1, checkout_theme = $2, checkout_message = $3, support_email = $4, \
               support_url = $5, return_url = $6, return_hosts = $7, updated_at = now() WHERE project_id = $8",
        )
        .bind(&b.accent_color)
        .bind(&b.theme)
        .bind(&b.message)
        .bind(&b.support_email)
        .bind(&b.support_url)
        .bind(&b.return_url)
        .bind(&b.return_hosts)
        .bind(project_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Store a processed logo (PNG and its hash), or remove it with `None`.
    pub async fn set_logo(&self, project_id: i64, logo: Option<(&[u8], &str)>, logo_url: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE projects SET logo_png = $1, logo_hash = $2, logo_url = $3, updated_at = now() WHERE project_id = $4")
            .bind(logo.map(|l| l.0))
            .bind(logo.map(|l| l.1))
            .bind(logo_url)
            .bind(project_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The logo PNG when `hash` is the current one.
    pub async fn logo_png(&self, project_id: i64, hash: &str) -> Result<Option<Vec<u8>>> {
        Ok(sqlx::query_scalar("SELECT logo_png FROM projects WHERE project_id = $1 AND logo_hash = $2")
            .bind(project_id)
            .bind(hash)
            .fetch_optional(&self.pool)
            .await?
            .flatten())
    }

    // ---- checkout domains ----------------------------------------------------

    pub async fn set_checkout_domain(&self, project_id: i64, domain: Option<&str>) -> Result<()> {
        let res = sqlx::query("UPDATE projects SET checkout_domain = $1, updated_at = now() WHERE project_id = $2")
            .bind(domain)
            .bind(project_id)
            .execute(&self.pool)
            .await;
        match res {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                Err(Error::Conflict("Another project uses this checkout domain.".into()))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// The project whose checkout domain is `host` (lowercase, no port).
    pub async fn project_for_checkout_domain(&self, host: &str) -> Result<Option<i64>> {
        Ok(sqlx::query_scalar("SELECT project_id FROM projects WHERE checkout_domain = $1").bind(host).fetch_optional(&self.pool).await?)
    }

    pub async fn checkout_domain(&self, project_id: i64) -> Result<Option<String>> {
        Ok(sqlx::query_scalar("SELECT checkout_domain FROM projects WHERE project_id = $1")
            .bind(project_id)
            .fetch_optional(&self.pool)
            .await?
            .flatten())
    }

    // ---- payment extras ----------------------------------------------------

    /// When the payment's status last changed, and the return URL it was created with.
    pub async fn payment_checkout_facts(&self, payment_id: i64) -> Result<(DateTime<Utc>, Option<String>)> {
        let row = sqlx::query("SELECT status_changed_at, return_url FROM payments WHERE payment_id = $1")
            .bind(payment_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)?;
        Ok((row.get("status_changed_at"), row.get("return_url")))
    }

    // ---- exchange settings ---------------------------------------------------

    pub async fn exchange_settings(&self, project_id: i64) -> Result<ExchangeSettings> {
        let r = sqlx::query(
            "SELECT exchange_enabled, trocador_key, exchange_markup, exchange_min_kyc, exchange_coins \
             FROM projects WHERE project_id = $1",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(Error::NotFound)?;
        Ok(ExchangeSettings {
            enabled: r.get("exchange_enabled"),
            key_sealed: r.get("trocador_key"),
            markup: r.get("exchange_markup"),
            min_kyc: r.get("exchange_min_kyc"),
            coins: r.get("exchange_coins"),
        })
    }

    /// Save the exchange settings. `key_sealed` of `None` keeps the stored key.
    pub async fn save_exchange_settings(
        &self,
        project_id: i64,
        enabled: bool,
        key_sealed: Option<&[u8]>,
        markup: Decimal,
        min_kyc: &str,
        coins: &[String],
    ) -> Result<()> {
        sqlx::query(
            "UPDATE projects SET exchange_enabled = $1, trocador_key = COALESCE($2, trocador_key), exchange_markup = $3, \
               exchange_min_kyc = $4, exchange_coins = $5, updated_at = now() WHERE project_id = $6",
        )
        .bind(enabled)
        .bind(key_sealed)
        .bind(markup)
        .bind(min_kyc)
        .bind(coins)
        .bind(project_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn clear_exchange_key(&self, project_id: i64) -> Result<()> {
        sqlx::query("UPDATE projects SET trocador_key = NULL, exchange_enabled = false, updated_at = now() WHERE project_id = $1")
            .bind(project_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// A sealed exchange key of any project that has one, for install-wide calls.
    pub async fn any_exchange_key(&self) -> Result<Option<Vec<u8>>> {
        Ok(sqlx::query_scalar(
            "SELECT trocador_key FROM projects WHERE trocador_key IS NOT NULL ORDER BY exchange_enabled DESC, project_id LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    // ---- exchange coins ------------------------------------------------------

    pub async fn replace_exchange_coins(&self, coins: &[ExchangeCoin]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM exchange_coins").execute(&mut *tx).await?;
        for c in coins {
            sqlx::query(
                "INSERT INTO exchange_coins (ticker, network, name, memo, minimum, maximum) VALUES ($1,$2,$3,$4,$5,$6) \
                 ON CONFLICT (ticker, network) DO NOTHING",
            )
            .bind(&c.ticker)
            .bind(&c.network)
            .bind(&c.name)
            .bind(c.memo)
            .bind(c.minimum)
            .bind(c.maximum)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// When the coin list was last refreshed.
    pub async fn exchange_coins_updated(&self) -> Result<Option<DateTime<Utc>>> {
        Ok(sqlx::query_scalar("SELECT MAX(updated_at) FROM exchange_coins").fetch_one(&self.pool).await?)
    }

    pub async fn exchange_coins(&self) -> Result<Vec<ExchangeCoin>> {
        let rows = sqlx::query("SELECT ticker, network, name, memo, minimum, maximum FROM exchange_coins ORDER BY name, ticker, network")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(coin_from_row).collect())
    }

    pub async fn exchange_coin(&self, ticker: &str, network: &str) -> Result<Option<ExchangeCoin>> {
        let row = sqlx::query(
            "SELECT ticker, network, name, memo, minimum, maximum FROM exchange_coins \
             WHERE LOWER(ticker) = LOWER($1) AND LOWER(network) = LOWER($2)",
        )
        .bind(ticker)
        .bind(network)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.as_ref().map(coin_from_row))
    }

    // ---- exchange trades -----------------------------------------------------

    pub async fn insert_trade(&self, t: &NewTrade<'_>) -> Result<TradeRow> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO exchange_trades (payment_id, project_id, token, hook_secret, webhook_key, trade_id, provider, \
               kyc_rating, coin_from, ticker_from, network_from, ticker_to, network_to, amount_from, amount_to, \
               address_provider, address_provider_memo, refund_address, refund_memo, status, expires_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21) RETURNING {TRADE_COLS}"
        )))
        .bind(t.payment_id)
        .bind(t.project_id)
        .bind(t.token)
        .bind(t.hook_secret)
        .bind(t.webhook_key)
        .bind(t.trade_id)
        .bind(t.provider)
        .bind(t.kyc_rating)
        .bind(t.coin_from)
        .bind(t.ticker_from)
        .bind(t.network_from)
        .bind(t.ticker_to)
        .bind(t.network_to)
        .bind(t.amount_from)
        .bind(t.amount_to)
        .bind(t.address_provider)
        .bind(t.address_provider_memo)
        .bind(t.refund_address)
        .bind(t.refund_memo)
        .bind(t.status)
        .bind(t.expires_at)
        .fetch_one(&self.pool)
        .await?;
        Ok(trade_from_row(&row))
    }

    pub async fn trade_by_token(&self, token: &str) -> Result<Option<TradeRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {TRADE_COLS} FROM exchange_trades WHERE token = $1")))
            .bind(token)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(trade_from_row))
    }

    pub async fn trade_by_hook_secret(&self, secret: &str) -> Result<Option<TradeRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {TRADE_COLS} FROM exchange_trades WHERE hook_secret = $1")))
            .bind(secret)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(trade_from_row))
    }

    /// Trades of a payment, newest first.
    pub async fn trades_for_payment(&self, payment_id: i64) -> Result<Vec<TradeRow>> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {TRADE_COLS} FROM exchange_trades WHERE payment_id = $1 ORDER BY created_at DESC LIMIT 20"
        )))
        .bind(payment_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(trade_from_row).collect())
    }

    /// Open trades not polled in the last `min_age_secs` seconds.
    pub async fn due_trades(&self, min_age_secs: i64, limit: i64) -> Result<Vec<TradeRow>> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {TRADE_COLS} FROM exchange_trades \
             WHERE status IN ('new','waiting','confirming','sending') \
               AND (polled_at IS NULL OR polled_at < now() - make_interval(secs => $1)) \
               AND created_at > now() - interval '3 days' \
             ORDER BY polled_at NULLS FIRST LIMIT $2"
        )))
        .bind(min_age_secs as f64)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(trade_from_row).collect())
    }

    /// Save a polled status. Returns true when the status changed.
    pub async fn update_trade_status(&self, exchange_trade_id: i64, status: &str, hashout: Option<&str>) -> Result<bool> {
        let changed: Option<bool> = sqlx::query_scalar(
            "UPDATE exchange_trades t SET \
               status_changed_at = CASE WHEN t.status <> $1 THEN now() ELSE t.status_changed_at END, \
               status = $1, hashout = COALESCE($2, t.hashout), polled_at = now(), updated_at = now() \
             FROM (SELECT status AS old FROM exchange_trades WHERE exchange_trade_id = $3) o \
             WHERE t.exchange_trade_id = $3 RETURNING o.old <> $1",
        )
        .bind(status)
        .bind(hashout)
        .bind(exchange_trade_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(changed.unwrap_or(false))
    }

    pub async fn touch_trade(&self, exchange_trade_id: i64) -> Result<()> {
        sqlx::query("UPDATE exchange_trades SET polled_at = now() WHERE exchange_trade_id = $1")
            .bind(exchange_trade_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
