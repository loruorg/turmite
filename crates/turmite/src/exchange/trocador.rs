//! Client for the Trocador API (rates and swaps across many exchanges).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::error::{Error, Result};

/// The public API. Tests and the demo pass another base to [`Trocador::with_base`].
pub const BASE_URL: &str = "https://api.trocador.app";

#[derive(Clone)]
pub struct Trocador {
    http: reqwest::Client,
    base: String,
    key: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Coin {
    pub name: String,
    pub ticker: String,
    pub network: String,
    pub memo: bool,
    pub minimum: Option<Decimal>,
    pub maximum: Option<Decimal>,
}

/// A quote request for a payment: the amount the payment must receive is fixed.
#[derive(Clone, Debug)]
pub struct RateRequest<'a> {
    pub ticker_from: &'a str,
    pub network_from: &'a str,
    pub ticker_to: &'a str,
    pub network_to: &'a str,
    pub amount_to: Decimal,
    pub min_kyc: &'a str,
    pub markup: Decimal,
}

#[derive(Clone, Debug)]
pub struct Rate {
    pub rate_id: String,
    pub provider: String,
    pub kyc_rating: Option<String>,
    pub amount_from: Decimal,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct TradeRequest<'a> {
    pub rate: &'a Rate,
    pub quote: &'a RateRequest<'a>,
    pub address: &'a str,
    pub address_memo: Option<&'a str>,
    pub refund: Option<&'a str>,
    pub refund_memo: Option<&'a str>,
    pub webhook: &'a str,
    pub webhook_key: &'a str,
}

#[derive(Clone, Debug, Default)]
pub struct Trade {
    pub trade_id: String,
    pub status: String,
    pub provider: String,
    pub amount_from: Decimal,
    pub amount_to: Decimal,
    pub address_provider: String,
    pub address_provider_memo: Option<String>,
    pub hashout: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Python-style booleans, as the API expects.
fn py(b: bool) -> String {
    if b { "True".into() } else { "False".into() }
}

fn text(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn dec(v: &Value, key: &str) -> Option<Decimal> {
    match v.get(key)? {
        Value::String(s) => s.trim().parse().ok(),
        Value::Number(n) => n.to_string().parse().ok().or_else(|| n.as_f64().and_then(|f| Decimal::try_from(f).ok())),
        _ => None,
    }
}

fn truthy(v: &Value, key: &str) -> bool {
    match v.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

fn date(v: &Value) -> Option<DateTime<Utc>> {
    let s = v.as_str()?;
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .ok()
        .or_else(|| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").ok().map(|n| n.and_utc()))
        .or_else(|| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f").ok().map(|n| n.and_utc()))
}

/// `quotes` is a list, or an object holding `quotes` and `expiresAt`.
fn quotes(v: &Value) -> (Vec<Value>, Option<DateTime<Utc>>) {
    match v.get("quotes") {
        Some(Value::Array(a)) => (a.clone(), None),
        Some(o @ Value::Object(_)) => {
            let list = o.get("quotes").and_then(Value::as_array).cloned().unwrap_or_default();
            (list, o.get("expiresAt").and_then(date))
        }
        _ => (vec![], None),
    }
}

fn parse_trade(v: &Value) -> Result<Trade> {
    let v = match v {
        Value::Array(a) => a.first().cloned().unwrap_or(Value::Null),
        other => other.clone(),
    };
    let trade_id = text(&v, "trade_id").ok_or_else(|| Error::Unavailable("The exchange returned no trade.".into()))?;
    Ok(Trade {
        trade_id,
        status: text(&v, "status").unwrap_or_else(|| "waiting".into()).to_ascii_lowercase(),
        provider: text(&v, "provider").unwrap_or_default(),
        amount_from: dec(&v, "amount_from").unwrap_or_default(),
        amount_to: dec(&v, "amount_to").unwrap_or_default(),
        address_provider: text(&v, "address_provider").unwrap_or_default(),
        address_provider_memo: text(&v, "address_provider_memo").filter(|m| m != "0" && !m.eq_ignore_ascii_case("none")),
        hashout: v.get("details").and_then(|d| text(d, "hashout")),
        expires_at: quotes(&v).1.or_else(|| v.get("expiresAt").and_then(date)),
    })
}

impl Trocador {
    pub fn new(http: reqwest::Client, key: &str) -> Self {
        Self::with_base(http, BASE_URL, key)
    }

    pub fn with_base(http: reqwest::Client, base: &str, key: &str) -> Self {
        Self { http, base: base.trim_end_matches('/').to_string(), key: key.to_string() }
    }

    async fn get(&self, method: &str, query: &[(&str, String)]) -> Result<Value> {
        let url = format!("{}/{method}", self.base);
        let resp = self
            .http
            .get(&url)
            .header("API-Key", &self.key)
            .query(query)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| Error::Unavailable(format!("exchange {method}: {e}")))?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let msg = text(&body, "error").or_else(|| text(&body, "message")).unwrap_or_default();
            tracing::warn!(method, status = status.as_u16(), message = %msg, "exchange request failed");
            return Err(match status.as_u16() {
                401 | 403 => Error::Unavailable("The exchange refused the API key.".into()),
                _ => Error::BadRequest("The exchange cannot do this swap now. Choose another coin or pay directly.".into()),
            });
        }
        if let Some(err) = text(&body, "error") {
            tracing::warn!(method, message = %err, "exchange error");
            return Err(Error::BadRequest("The exchange cannot do this swap now. Choose another coin or pay directly.".into()));
        }
        Ok(body)
    }

    pub async fn coins(&self) -> Result<Vec<Coin>> {
        let body = self.get("coins", &[]).await?;
        let list = body.as_array().cloned().unwrap_or_default();
        Ok(list
            .iter()
            .filter_map(|c| {
                Some(Coin {
                    name: text(c, "name")?,
                    ticker: text(c, "ticker")?.to_ascii_lowercase(),
                    network: text(c, "network")?,
                    memo: truthy(c, "memo"),
                    minimum: dec(c, "minimum"),
                    maximum: dec(c, "maximum"),
                })
            })
            .collect())
    }

    pub async fn new_rate(&self, r: &RateRequest<'_>) -> Result<Rate> {
        let q = [
            ("ticker_from", r.ticker_from.to_string()),
            ("network_from", r.network_from.to_string()),
            ("ticker_to", r.ticker_to.to_string()),
            ("network_to", r.network_to.to_string()),
            ("amount_to", r.amount_to.normalize().to_string()),
            ("payment", py(true)),
            ("min_kycrating", r.min_kyc.to_string()),
            ("markup", r.markup.normalize().to_string()),
        ];
        let v = self.get("new_rate", &q).await?;
        let rate_id = text(&v, "trade_id").ok_or_else(|| Error::BadRequest("The exchange has no quote for this coin.".into()))?;
        let provider = text(&v, "provider").ok_or_else(|| Error::BadRequest("The exchange has no quote for this coin.".into()))?;
        let (list, expires_at) = quotes(&v);
        let best = list.iter().find(|q| text(q, "provider").is_some_and(|p| p == provider));
        let kyc_rating = best.and_then(|q| text(q, "kycrating").or_else(|| text(q, "kyc_rating")));
        let amount_from = dec(&v, "amount_from")
            .filter(|a| *a > Decimal::ZERO)
            .or_else(|| best.and_then(|q| dec(q, "amount_from")))
            .ok_or_else(|| Error::BadRequest("The exchange has no quote for this coin.".into()))?;
        Ok(Rate { rate_id, provider, kyc_rating, amount_from, expires_at })
    }

    pub async fn new_trade(&self, t: &TradeRequest<'_>) -> Result<Trade> {
        let mut q = vec![
            ("id", t.rate.rate_id.clone()),
            ("ticker_from", t.quote.ticker_from.to_string()),
            ("network_from", t.quote.network_from.to_string()),
            ("ticker_to", t.quote.ticker_to.to_string()),
            ("network_to", t.quote.network_to.to_string()),
            ("amount_to", t.quote.amount_to.normalize().to_string()),
            ("address", t.address.to_string()),
            ("address_memo", t.address_memo.filter(|m| !m.is_empty()).unwrap_or("0").to_string()),
            ("provider", t.rate.provider.clone()),
            ("fixed", py(true)),
            ("payment", py(true)),
            ("min_kycrating", t.quote.min_kyc.to_string()),
            ("markup", t.quote.markup.normalize().to_string()),
            ("webhook", t.webhook.to_string()),
            ("webhook_key", t.webhook_key.to_string()),
        ];
        if let Some(refund) = t.refund.filter(|r| !r.is_empty()) {
            q.push(("refund", refund.to_string()));
            q.push(("refund_memo", t.refund_memo.filter(|m| !m.is_empty()).unwrap_or("0").to_string()));
        }
        let v = self.get("new_trade", &q).await?;
        let mut trade = parse_trade(&v)?;
        if trade.address_provider.is_empty() {
            return Err(Error::Unavailable("The exchange returned no deposit address.".into()));
        }
        if trade.expires_at.is_none() {
            trade.expires_at = t.rate.expires_at;
        }
        Ok(trade)
    }

    pub async fn trade(&self, id: &str) -> Result<Trade> {
        parse_trade(&self.get("trade", &[("id", id.to_string())]).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_trades_in_both_shapes() {
        let one = json!({"trade_id": "abc", "status": "Waiting", "amount_from": 1.5, "amount_to": "0.01",
            "address_provider": "LTCaddr", "address_provider_memo": "0", "quotes": {"expiresAt": "2026-01-01T10:00:00Z"},
            "details": {"hashout": "0xhash"}});
        let t = parse_trade(&one).unwrap();
        assert_eq!((t.trade_id.as_str(), t.status.as_str()), ("abc", "waiting"));
        assert_eq!(t.amount_from, Decimal::new(15, 1));
        assert_eq!(t.address_provider_memo, None);
        assert_eq!(t.hashout.as_deref(), Some("0xhash"));
        assert!(t.expires_at.is_some());
        let list = json!([{"trade_id": 42, "status": "finished"}]);
        assert_eq!(parse_trade(&list).unwrap().trade_id, "42");
        assert!(parse_trade(&json!([])).is_err());
    }
}
