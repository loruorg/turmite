//! Exchange rates. CoinGecko first, Coinbase as fallback, a 60-second cache.
//! Stablecoins count as 1 USD. When every source fails, quoting fails: a guessed
//! price would charge the customer the wrong amount.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use rust_decimal::{Decimal, RoundingStrategy};

use crate::{
    chains::catalog::ASSETS,
    error::{Error, Result},
};

#[async_trait]
pub trait RateProvider: Send + Sync {
    /// Price of one unit of the asset (`coingecko_id`) in `vs_currency` (lower case).
    async fn price(&self, coingecko_id: &str, vs_currency: &str) -> Result<Decimal>;
}

/// Amount of the asset that is worth `amount` of `from_currency`, rounded up to `decimals`.
pub async fn estimate(
    provider: &dyn RateProvider,
    amount: Decimal,
    from_currency: &str,
    coingecko_id: &str,
    decimals: u32,
) -> Result<(Decimal, Decimal)> {
    let price = provider.price(coingecko_id, &from_currency.to_ascii_lowercase()).await?;
    if price <= Decimal::ZERO {
        return Err(Error::Rate(format!("no price for {coingecko_id}")));
    }
    let out = (amount / price).round_dp_with_strategy(decimals.min(18), RoundingStrategy::AwayFromZero);
    Ok((out, price))
}

pub fn default_rate_provider() -> Arc<dyn RateProvider> {
    Arc::new(LiveRates::new())
}

fn is_stable(id: &str) -> bool {
    ASSETS.iter().any(|a| a.coingecko_id == id && a.is_stable)
}

fn symbol_of(id: &str) -> Option<&'static str> {
    ASSETS.iter().find(|a| a.coingecko_id == id).map(|a| a.symbol)
}

pub struct LiveRates {
    client: reqwest::Client,
    cache: Mutex<HashMap<(String, String), (Instant, Decimal)>>,
}

impl LiveRates {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(8))
                .user_agent(concat!("turmite/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("http client"),
            cache: Mutex::new(HashMap::new()),
        }
    }

    async fn coingecko(&self, id: &str, vs: &str) -> Result<Decimal> {
        let url = format!("https://api.coingecko.com/api/v3/simple/price?ids={id}&vs_currencies={vs}&precision=full");
        let body: serde_json::Value = self
            .client
            .get(url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| Error::Rate(format!("coingecko: {e}")))?
            .json()
            .await
            .map_err(|e| Error::Rate(format!("coingecko: {e}")))?;
        let v = body.get(id).and_then(|v| v.get(vs)).ok_or_else(|| Error::Rate(format!("coingecko: no {id}/{vs}")))?;
        parse_decimal(v).ok_or_else(|| Error::Rate("coingecko: bad number".into()))
    }

    async fn coinbase(&self, id: &str, vs: &str) -> Result<Decimal> {
        let symbol = symbol_of(id).ok_or_else(|| Error::Rate(format!("coinbase: no symbol for {id}")))?;
        let url = format!("https://api.coinbase.com/v2/prices/{symbol}-{}/spot", vs.to_ascii_uppercase());
        let body: serde_json::Value = self
            .client
            .get(url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| Error::Rate(format!("coinbase: {e}")))?
            .json()
            .await
            .map_err(|e| Error::Rate(format!("coinbase: {e}")))?;
        body.pointer("/data/amount").and_then(parse_decimal).ok_or_else(|| Error::Rate("coinbase: bad response".into()))
    }
}

impl Default for LiveRates {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_decimal(v: &serde_json::Value) -> Option<Decimal> {
    match v {
        serde_json::Value::String(s) => s.parse().ok(),
        serde_json::Value::Number(n) => n.to_string().parse().ok().or_else(|| n.as_f64().and_then(|f| Decimal::try_from(f).ok())),
        _ => None,
    }
}

#[async_trait]
impl RateProvider for LiveRates {
    async fn price(&self, id: &str, vs: &str) -> Result<Decimal> {
        let vs = vs.to_ascii_lowercase();
        if is_stable(id) && vs == "usd" {
            return Ok(Decimal::ONE);
        }
        let key = (id.to_string(), vs.clone());
        if let Some((at, price)) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key)
            && at.elapsed() < Duration::from_secs(60)
        {
            return Ok(*price);
        }
        let price = match self.coingecko(id, &vs).await {
            Ok(p) => p,
            Err(first) => self.coinbase(id, &vs).await.map_err(|second| Error::Rate(format!("{first}; {second}")))?,
        };
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).insert(key, (Instant::now(), price));
        Ok(price)
    }
}

/// Fixed prices for tests and `--demo`.
#[derive(Clone, Default)]
pub struct FixedRateProvider {
    pub rates: HashMap<(String, String), Decimal>,
}

impl FixedRateProvider {
    pub fn with(mut self, coingecko_id: &str, vs: &str, price: Decimal) -> Self {
        self.rates.insert((coingecko_id.into(), vs.into()), price);
        self
    }

    /// Round prices for every catalog asset in USD and EUR.
    pub fn demo() -> Self {
        let usd: &[(&str, i64)] = &[
            ("bitcoin", 100_000),
            ("monero", 300),
            ("ethereum", 4_000),
            ("polygon-ecosystem-token", 1),
            ("binancecoin", 1_000),
            ("tron", 1),
            ("solana", 200),
            ("the-open-network", 3),
            ("usd-coin", 1),
            ("tether", 1),
        ];
        let mut p = Self::default();
        for (id, price) in usd {
            p = p.with(id, "usd", Decimal::from(*price)).with(id, "eur", Decimal::from(*price) * Decimal::new(92, 2));
        }
        p
    }
}

#[async_trait]
impl RateProvider for FixedRateProvider {
    async fn price(&self, id: &str, vs: &str) -> Result<Decimal> {
        if is_stable(id) && vs == "usd" && !self.rates.contains_key(&(id.to_string(), vs.to_string())) {
            return Ok(Decimal::ONE);
        }
        self.rates.get(&(id.to_string(), vs.to_string())).copied().ok_or_else(|| Error::Rate(format!("no fixed rate for {id}/{vs}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn estimate_rounds_up() {
        let p = FixedRateProvider::default().with("bitcoin", "usd", Decimal::from(30_000));
        let (amount, price) = estimate(&p, Decimal::from(10), "USD", "bitcoin", 8).await.unwrap();
        assert_eq!(price, Decimal::from(30_000));
        assert_eq!(amount, "0.00033334".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn stablecoin_is_one_usd() {
        let p = FixedRateProvider::default();
        assert_eq!(p.price("tether", "usd").await.unwrap(), Decimal::ONE);
        assert!(p.price("bitcoin", "usd").await.is_err());
    }
}
