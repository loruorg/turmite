use rust_decimal::Decimal;
use sqlx::{Row, postgres::PgRow};

use super::PgStore;
use crate::{
    chains::catalog::{self, NetworkDef},
    config::Mode,
    domain::{Asset, AssetNetwork, Network},
    error::{Error, Result},
};

const NETWORK_COLS: &str = "n.network_id, n.code, n.chain, n.family, n.name, n.chain_id, n.kind::text AS kind, \
    n.native_asset_id, n.confirmations_required, n.explorer_tx_url, n.explorer_address_url, n.is_testnet, \
    n.is_enabled AS net_enabled, n.last_block, n.last_block_at";

const ASSET_NETWORK_SQL: &str = "SELECT an.asset_network_id, an.ticker, an.decimals, an.contract_address, \
    an.min_payment_amount, an.is_enabled, a.asset_id, a.symbol, a.name AS asset_name, a.coingecko_id, a.is_stable, \
    n.network_id, n.code, n.chain, n.family, n.name, n.chain_id, n.kind::text AS kind, n.native_asset_id, \
    n.confirmations_required, n.explorer_tx_url, n.explorer_address_url, n.is_testnet, n.is_enabled AS net_enabled, \
    n.last_block, n.last_block_at \
    FROM asset_network an JOIN asset a ON a.asset_id = an.asset_id JOIN network n ON n.network_id = an.network_id";

impl PgStore {
    /// Insert the catalog for `mode`. Existing rows keep operator edits.
    /// Refuses to run when the database was seeded for the other mode.
    pub async fn seed_catalog(&self, mode: Mode) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(7431001)").execute(&mut *tx).await?;
        let stored: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT value FROM settings WHERE key = 'mode'").fetch_optional(&mut *tx).await?;
        match stored.as_ref().and_then(|v| v.as_str()) {
            Some(m) if m != mode.as_str() => {
                return Err(Error::BadRequest(format!(
                    "this database was set up for {m}; TURMITE_MODE is {}. Use a new database for the other mode.",
                    mode.as_str()
                )));
            }
            Some(_) => {}
            None => {
                sqlx::query("INSERT INTO settings (key, value) VALUES ('mode', $1)")
                    .bind(serde_json::json!(mode.as_str()))
                    .execute(&mut *tx)
                    .await?;
            }
        }
        for a in catalog::ASSETS {
            sqlx::query(
                "INSERT INTO asset (symbol, name, coingecko_id, is_stable) VALUES ($1,$2,$3,$4) \
                 ON CONFLICT (LOWER(symbol)) DO NOTHING",
            )
            .bind(a.symbol)
            .bind(a.name)
            .bind(a.coingecko_id)
            .bind(a.is_stable)
            .execute(&mut *tx)
            .await?;
        }
        for n in catalog::networks(mode) {
            seed_network(&mut tx, &n, !mode.is_mainnet()).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn list_networks(&self) -> Result<Vec<Network>> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {NETWORK_COLS} FROM network n ORDER BY n.network_id")))
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(network_from_row).collect()
    }

    pub async fn network(&self, network_id: i64) -> Result<Network> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {NETWORK_COLS} FROM network n WHERE n.network_id = $1")))
            .bind(network_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)?;
        network_from_row(&row)
    }

    pub async fn network_by_chain(&self, chain: &str) -> Result<Option<Network>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {NETWORK_COLS} FROM network n WHERE n.chain = $1")))
            .bind(chain)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(network_from_row).transpose()
    }

    /// Every asset on every network, enabled or not.
    pub async fn list_asset_networks(&self) -> Result<Vec<AssetNetwork>> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!("{ASSET_NETWORK_SQL} ORDER BY n.network_id, an.asset_network_id")))
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(asset_network_from_row).collect()
    }

    pub async fn asset_network(&self, asset_network_id: i64) -> Result<AssetNetwork> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!("{ASSET_NETWORK_SQL} WHERE an.asset_network_id = $1")))
            .bind(asset_network_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)?;
        asset_network_from_row(&row)
    }

    /// Resolve an enabled ticker (`btc`, `usdttrc20`).
    pub async fn asset_network_by_ticker(&self, ticker: &str) -> Result<AssetNetwork> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!("{ASSET_NETWORK_SQL} WHERE an.ticker = $1 AND an.is_enabled AND n.is_enabled")))
            .bind(ticker.trim().to_ascii_lowercase())
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => asset_network_from_row(&r),
            None => Err(Error::BadRequest(format!("Currency `{ticker}` is not supported."))),
        }
    }

    pub async fn set_network_confirmations(&self, network_id: i64, confirmations: i32) -> Result<()> {
        sqlx::query("UPDATE network SET confirmations_required = $1 WHERE network_id = $2")
            .bind(confirmations)
            .bind(network_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_network_enabled(&self, network_id: i64, enabled: bool) -> Result<()> {
        sqlx::query("UPDATE network SET is_enabled = $1 WHERE network_id = $2").bind(enabled).bind(network_id).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn set_network_last_block(&self, network_id: i64, height: i64) -> Result<()> {
        sqlx::query("UPDATE network SET last_block = $1, last_block_at = now() WHERE network_id = $2")
            .bind(height)
            .bind(network_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Add or change a token on a network.
    pub async fn upsert_token(
        &self,
        network_id: i64,
        symbol: &str,
        ticker: &str,
        decimals: i32,
        contract: &str,
        min_payment_amount: Option<Decimal>,
    ) -> Result<i64> {
        let asset_id: i64 = sqlx::query_scalar("SELECT asset_id FROM asset WHERE LOWER(symbol) = LOWER($1)")
            .bind(symbol)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::BadRequest(format!("Unknown asset `{symbol}`.")))?;
        let id = sqlx::query_scalar(
            "INSERT INTO asset_network (asset_id, network_id, ticker, decimals, contract_address, min_payment_amount) \
             VALUES ($1,$2,$3,$4,$5,$6) \
             ON CONFLICT (network_id, asset_id) DO UPDATE SET ticker = EXCLUDED.ticker, decimals = EXCLUDED.decimals, \
               contract_address = EXCLUDED.contract_address, min_payment_amount = EXCLUDED.min_payment_amount \
             RETURNING asset_network_id",
        )
        .bind(asset_id)
        .bind(network_id)
        .bind(ticker.to_ascii_lowercase())
        .bind(decimals)
        .bind(contract)
        .bind(min_payment_amount)
        .fetch_one(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn set_asset_network_enabled(&self, asset_network_id: i64, enabled: bool) -> Result<()> {
        sqlx::query("UPDATE asset_network SET is_enabled = $1 WHERE asset_network_id = $2")
            .bind(enabled)
            .bind(asset_network_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

async fn seed_network(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, n: &NetworkDef, testnet: bool) -> Result<()> {
    let native_asset_id: i64 =
        sqlx::query_scalar("SELECT asset_id FROM asset WHERE LOWER(symbol) = LOWER($1)").bind(n.native_symbol).fetch_one(&mut **tx).await?;
    let network_id: i64 = sqlx::query_scalar(
        "INSERT INTO network (code, chain, family, name, chain_id, kind, native_asset_id, confirmations_required, \
           explorer_tx_url, explorer_address_url, is_testnet) \
         VALUES ($1,$2,$3,$4,$5,$6::network_kind,$7,$8,$9,$10,$11) \
         ON CONFLICT (code) DO UPDATE SET name = EXCLUDED.name, explorer_tx_url = EXCLUDED.explorer_tx_url, \
           explorer_address_url = EXCLUDED.explorer_address_url \
         RETURNING network_id",
    )
    .bind(n.code)
    .bind(n.chain)
    .bind(n.family)
    .bind(n.name)
    .bind(n.chain_id)
    .bind(n.kind)
    .bind(native_asset_id)
    .bind(n.confirmations)
    .bind(n.explorer_tx)
    .bind(n.explorer_address)
    .bind(testnet)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO asset_network (asset_id, network_id, ticker, decimals, contract_address) VALUES ($1,$2,$3,$4,NULL) \
         ON CONFLICT DO NOTHING",
    )
    .bind(native_asset_id)
    .bind(network_id)
    .bind(n.native_ticker)
    .bind(n.native_decimals)
    .execute(&mut **tx)
    .await?;
    for t in &n.tokens {
        sqlx::query(
            "INSERT INTO asset_network (asset_id, network_id, ticker, decimals, contract_address) \
             SELECT asset_id, $1, $2, $3, $4 FROM asset WHERE LOWER(symbol) = LOWER($5) \
             ON CONFLICT DO NOTHING",
        )
        .bind(network_id)
        .bind(t.ticker)
        .bind(t.decimals)
        .bind(t.contract)
        .bind(t.symbol)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

pub(crate) fn network_from_row(r: &PgRow) -> Result<Network> {
    Ok(Network {
        network_id: r.get("network_id"),
        code: r.get("code"),
        chain: r.get("chain"),
        family: r.get("family"),
        name: r.get("name"),
        chain_id: r.get("chain_id"),
        kind: r.get::<String, _>("kind").parse()?,
        native_asset_id: r.get("native_asset_id"),
        confirmations_required: r.get("confirmations_required"),
        explorer_tx_url: r.get("explorer_tx_url"),
        explorer_address_url: r.get("explorer_address_url"),
        is_testnet: r.get("is_testnet"),
        is_enabled: r.get("net_enabled"),
        last_block: r.get("last_block"),
        last_block_at: r.get("last_block_at"),
    })
}

fn asset_network_from_row(r: &PgRow) -> Result<AssetNetwork> {
    Ok(AssetNetwork {
        asset_network_id: r.get("asset_network_id"),
        asset: Asset {
            asset_id: r.get("asset_id"),
            symbol: r.get("symbol"),
            name: r.get("asset_name"),
            coingecko_id: r.get("coingecko_id"),
            is_stable: r.get("is_stable"),
        },
        network: network_from_row(r)?,
        ticker: r.get("ticker"),
        decimals: r.get("decimals"),
        contract_address: r.get("contract_address"),
        min_payment_amount: r.get("min_payment_amount"),
        is_enabled: r.get("is_enabled"),
    })
}
