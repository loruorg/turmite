//! Pay with any coin: the customer swaps another coin into the payment's own coin through
//! Trocador. The exchange sends to the payment's deposit address and the chain watcher
//! credits what arrives. Swap statuses are shown on the checkout only.

pub mod stub;
pub mod trocador;

use std::time::Duration;

use chrono::Utc;
use rust_decimal::Decimal;

use crate::{
    app::App,
    db::{ExchangeCoin, ExchangeSettings, NewTrade, PaymentRow, TradeRow},
    domain::{AssetNetwork, PaymentStatus},
    error::{Error, Result},
    ids,
};
pub use trocador::Trocador;

/// Markups the API accepts, in percent.
pub const MARKUPS: [&str; 4] = ["0", "1", "1.65", "3"];
pub const KYC_RATINGS: [&str; 4] = ["A", "B", "C", "D"];
/// Swap statuses that can still change.
pub const OPEN_STATUSES: [&str; 4] = ["new", "waiting", "confirming", "sending"];

/// Exchange ticker and network candidates for each payment ticker. The first one the
/// exchange lists wins.
const ALIASES: &[(&str, &[(&str, &str)])] = &[
    ("btc", &[("btc", "Mainnet")]),
    ("xmr", &[("xmr", "Mainnet")]),
    ("eth", &[("eth", "ERC20"), ("eth", "Mainnet")]),
    ("usdc", &[("usdc", "ERC20")]),
    ("usdterc20", &[("usdt", "ERC20")]),
    ("ethbase", &[("eth", "Base"), ("ethbase", "Mainnet")]),
    ("usdcbase", &[("usdc", "Base")]),
    ("pol", &[("pol", "Polygon"), ("pol", "Mainnet"), ("matic", "Polygon")]),
    ("usdcmatic", &[("usdc", "Polygon"), ("usdc", "MATIC")]),
    ("usdtmatic", &[("usdt", "Polygon"), ("usdt", "MATIC")]),
    ("bnbbsc", &[("bnb", "BEP20"), ("bnb", "BSC")]),
    ("usdcbsc", &[("usdc", "BEP20"), ("usdc", "BSC")]),
    ("usdtbsc", &[("usdt", "BEP20"), ("usdt", "BSC")]),
    ("trx", &[("trx", "Mainnet"), ("trx", "TRC20")]),
    ("usdttrc20", &[("usdt", "TRC20")]),
    ("sol", &[("sol", "Mainnet"), ("sol", "Solana")]),
    ("usdcsol", &[("usdc", "Solana"), ("usdc", "SOL"), ("usdc", "SPL")]),
    ("usdtsol", &[("usdt", "Solana"), ("usdt", "SOL"), ("usdt", "SPL")]),
    ("ton", &[("ton", "Mainnet"), ("ton", "TON")]),
];

/// Payment coins in the order the exchange option of an invoice prefers them.
pub const SETTLE_PREFERENCE: [&str; 19] = [
    "usdttrc20",
    "usdtsol",
    "usdcsol",
    "usdcbase",
    "usdtbsc",
    "usdcbsc",
    "usdtmatic",
    "usdcmatic",
    "usdc",
    "usdterc20",
    "btc",
    "xmr",
    "eth",
    "ethbase",
    "sol",
    "trx",
    "ton",
    "bnbbsc",
    "pol",
];

/// The exchange's coin for a payment coin, verified against the listed coins.
pub async fn target_coin(app: &App, an: &AssetNetwork) -> Result<Option<ExchangeCoin>> {
    let Some((_, candidates)) = ALIASES.iter().find(|(t, _)| *t == an.ticker) else { return Ok(None) };
    for (ticker, network) in candidates.iter() {
        if let Some(c) = app.store.exchange_coin(ticker, network).await? {
            return Ok(Some(c));
        }
    }
    Ok(None)
}

/// The exchange works on mainnet, or against a stub given at start (tests, demo).
pub fn supported(app: &App) -> bool {
    app.config.mode.is_mainnet() || app.exchange_base.is_some()
}

fn client_with(app: &App, key: &str) -> Trocador {
    match &app.exchange_base {
        Some(base) => Trocador::with_base(app.http.clone(), base, key),
        None => Trocador::new(app.http.clone(), key),
    }
}

/// A client with the project's key, when the exchange is on for it.
pub fn client(app: &App, s: &ExchangeSettings) -> Result<Option<Trocador>> {
    if !s.enabled || !supported(app) {
        return Ok(None);
    }
    let Some(sealed) = &s.key_sealed else { return Ok(None) };
    let key = app.sealer.open_str("trocador_key", sealed)?;
    Ok(Some(client_with(app, &key)))
}

/// A client with a key given in the dashboard, to check it before it is saved.
pub fn client_for_key(app: &App, key: &str) -> Trocador {
    client_with(app, key)
}

/// True when the allow list is empty or names this coin (`ticker` or `ticker:network`).
pub fn allowed(s: &ExchangeSettings, c: &ExchangeCoin) -> bool {
    s.coins.is_empty()
        || s.coins.iter().any(|a| {
            let a = a.to_ascii_lowercase();
            match a.split_once(':') {
                Some((t, n)) => t == c.ticker.to_ascii_lowercase() && n == c.network.to_ascii_lowercase(),
                None => a == c.ticker.to_ascii_lowercase(),
            }
        })
}

/// Download the coin list and store it.
pub async fn refresh_coins_with(app: &App, client: &Trocador) -> Result<usize> {
    let coins = client.coins().await?;
    if coins.is_empty() {
        return Err(Error::Unavailable("The exchange listed no coins.".into()));
    }
    let rows: Vec<ExchangeCoin> = coins
        .into_iter()
        .map(|c| ExchangeCoin { ticker: c.ticker, network: c.network, name: c.name, memo: c.memo, minimum: c.minimum, maximum: c.maximum })
        .collect();
    app.store.replace_exchange_coins(&rows).await?;
    Ok(rows.len())
}

/// Refresh the coin list once a day with any project's key. After a failure, wait an hour.
pub async fn refresh_coins_if_stale(app: &App) -> Result<()> {
    static LAST_TRY: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    if !supported(app) {
        return Ok(());
    }
    let stale = app.store.exchange_coins_updated().await?.is_none_or(|t| Utc::now() - t > chrono::Duration::hours(24));
    if !stale {
        return Ok(());
    }
    {
        let mut last = LAST_TRY.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|t| t.elapsed() < Duration::from_secs(3600)) {
            return Ok(());
        }
        *last = Some(std::time::Instant::now());
    }
    let Some(sealed) = app.store.any_exchange_key().await? else { return Ok(()) };
    let key = app.sealer.open_str("trocador_key", &sealed)?;
    refresh_coins_with(app, &client_with(app, &key)).await.map(|_| ())
}

/// Amount the payment still needs.
pub fn amount_due(p: &PaymentRow) -> Decimal {
    (p.pay_amount - p.actually_paid).max(Decimal::ZERO)
}

/// Payment states in which a swap can still be started.
pub fn payment_open(p: &PaymentRow) -> bool {
    matches!(p.status, PaymentStatus::Waiting | PaymentStatus::PartiallyPaid) && p.expires_at > Utc::now()
}

/// Refuse a swap when the amount to send is outside the coin's range at the exchange.
/// The range is in units of the coin. A zero amount (not in the quote) is not checked.
fn check_limits(coin: &ExchangeCoin, amount: Decimal) -> Result<()> {
    if amount <= Decimal::ZERO {
        return Ok(());
    }
    let ticker = coin.ticker.to_ascii_uppercase();
    let amount_text = amount.normalize();
    if let Some(min) = coin.minimum.filter(|m| amount < *m) {
        return Err(Error::BadRequest(format!(
            "The amount to send, {amount_text} {ticker}, is less than the exchange minimum of {} {ticker}. Choose another coin.",
            min.normalize()
        )));
    }
    if let Some(max) = coin.maximum.filter(|m| *m > Decimal::ZERO && amount > *m) {
        return Err(Error::BadRequest(format!(
            "The amount to send, {amount_text} {ticker}, is more than the exchange maximum of {} {ticker}. Choose another coin.",
            max.normalize()
        )));
    }
    Ok(())
}

/// Ask for the best fixed-rate quote and start a swap into the payment's deposit address.
pub async fn start_trade(
    app: &App,
    p: &PaymentRow,
    from: &ExchangeCoin,
    refund: Option<&str>,
    refund_memo: Option<&str>,
) -> Result<TradeRow> {
    let settings = app.store.exchange_settings(p.project_id).await?;
    let client = client(app, &settings)?.ok_or_else(|| Error::BadRequest("This shop does not take other coins.".into()))?;
    if !allowed(&settings, from) {
        return Err(Error::BadRequest("This shop does not take this coin.".into()));
    }
    if !payment_open(p) {
        return Err(Error::BadRequest("This payment is closed.".into()));
    }
    let an = app.store.asset_network(p.asset_network_id).await?;
    let to = target_coin(app, &an).await?.ok_or_else(|| Error::BadRequest("The exchange cannot send this payment's coin.".into()))?;
    let due = amount_due(p);
    if due <= Decimal::ZERO {
        return Err(Error::BadRequest("Nothing is left to pay.".into()));
    }
    let quote = trocador::RateRequest {
        ticker_from: &from.ticker,
        network_from: &from.network,
        ticker_to: &to.ticker,
        network_to: &to.network,
        amount_to: due,
        min_kyc: &settings.min_kyc,
        markup: settings.markup,
    };
    let rate = client.new_rate(&quote).await?;
    check_limits(from, rate.amount_from)?;
    let hook_secret = ids::token();
    let webhook_key = crate::crypto::random_hex(24);
    // The checkout domain is public by definition; the site URL can be private (a dashboard
    // on a tailnet name). The hook works on both hosts.
    let domain = app.store.checkout_domain(p.project_id).await?;
    let webhook = format!("{}/x/hook/{hook_secret}", crate::http::checkout_base(app, domain.as_deref()));
    let trade = client
        .new_trade(&trocador::TradeRequest {
            rate: &rate,
            quote: &quote,
            address: &p.pay_address,
            address_memo: p.payin_extra_id.as_deref(),
            refund,
            refund_memo,
            webhook: &webhook,
            webhook_key: &webhook_key,
        })
        .await?;
    let amount_from = if trade.amount_from > Decimal::ZERO { trade.amount_from } else { rate.amount_from };
    app.store
        .insert_trade(&NewTrade {
            payment_id: p.payment_id,
            project_id: p.project_id,
            token: &ids::token(),
            hook_secret: &hook_secret,
            webhook_key: &webhook_key,
            trade_id: &trade.trade_id,
            provider: if trade.provider.is_empty() { &rate.provider } else { &trade.provider },
            kyc_rating: rate.kyc_rating.as_deref(),
            coin_from: &from.name,
            ticker_from: &from.ticker,
            network_from: &from.network,
            ticker_to: &to.ticker,
            network_to: &to.network,
            amount_from,
            amount_to: due,
            address_provider: &trade.address_provider,
            address_provider_memo: trade.address_provider_memo.as_deref(),
            refund_address: refund.filter(|r| !r.is_empty()),
            refund_memo: refund_memo.filter(|r| !r.is_empty()),
            status: &trade.status,
            expires_at: trade.expires_at.or(rate.expires_at),
        })
        .await
}

/// `--demo`: turn the exchange on for the demo project against the local stub.
pub async fn demo_setup(app: &App, project_id: i64) -> Result<()> {
    if app.exchange_base.is_none() {
        return Ok(());
    }
    let sealed = app.sealer.seal_str("trocador_key", "demo");
    app.store.save_exchange_settings(project_id, true, Some(&sealed), Decimal::ZERO, "B", &[]).await?;
    refresh_coins_with(app, &client_with(app, "demo")).await.map(|_| ())
}

/// Fetch a swap's status from the exchange and save it.
pub async fn refresh_trade(app: &App, t: &TradeRow) -> Result<()> {
    let settings = app.store.exchange_settings(t.project_id).await?;
    let client = match &settings.key_sealed {
        Some(sealed) if supported(app) => client_with(app, &app.sealer.open_str("trocador_key", sealed)?),
        _ => return Ok(()),
    };
    let remote = match client.trade(&t.trade_id).await {
        Ok(r) => r,
        Err(e) => {
            app.store.touch_trade(t.exchange_trade_id).await?;
            return Err(e);
        }
    };
    let status = normalize_status(&remote.status);
    let changed = app.store.update_trade_status(t.exchange_trade_id, status, remote.hashout.as_deref()).await?;
    if changed {
        tracing::info!(trade = %t.trade_id, payment_id = t.payment_id, status, "swap status changed");
        if app.demo && status == "finished" {
            demo_deliver(app, t).await?;
        }
    }
    Ok(())
}

/// Known swap statuses; anything else shows as waiting.
pub fn normalize_status(s: &str) -> &'static str {
    match s.trim().to_ascii_lowercase().as_str() {
        "new" => "new",
        "confirming" => "confirming",
        "sending" => "sending",
        "finished" => "finished",
        "failed" => "failed",
        "expired" => "expired",
        "halted" => "halted",
        "refunded" => "refunded",
        "paid partially" | "paid_partially" => "confirming",
        _ => "waiting",
    }
}

/// `--demo` only: the simulated exchange sends the coins to the payment's simulated chain.
async fn demo_deliver(app: &App, t: &TradeRow) -> Result<()> {
    let Some(p) = app.store.payment_any(t.payment_id).await? else { return Ok(()) };
    let network = app.store.network(p.network_id).await?;
    let drivers = app.drivers();
    let Some(driver) = drivers.get(&network.chain) else { return Ok(()) };
    if let Some(mock) = crate::chains::driver::as_mock(&driver) {
        let txid = format!("swap-{}", t.trade_id);
        mock.inject(
            &p.pay_address,
            crate::chains::driver::Observation {
                chain_tx_key: txid.clone(),
                txid,
                vout: None,
                amount: t.amount_to,
                confirmations: network.confirmations_required.max(1),
                block_height: None,
            },
        )
        .await;
    }
    Ok(())
}

/// One pass: refresh open swaps, then the coin list when it is a day old.
pub async fn poll(app: &App) {
    match app.store.due_trades(20, 50).await {
        Ok(trades) => {
            for t in trades {
                if let Err(e) = refresh_trade(app, &t).await {
                    tracing::warn!(trade = %t.trade_id, error = %e, "swap status poll failed");
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "listing open swaps failed"),
    }
    if let Err(e) = refresh_coins_if_stale(app).await {
        tracing::warn!(error = %e, "exchange coin list refresh failed");
    }
}

/// Worker loop for swap statuses and the coin list.
pub fn spawn(app: App) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            poll(&app).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_list() {
        let coin = ExchangeCoin {
            ticker: "usdt".into(),
            network: "TRC20".into(),
            name: "Tether".into(),
            memo: false,
            minimum: None,
            maximum: None,
        };
        let mut s = ExchangeSettings { enabled: true, key_sealed: None, markup: Decimal::ZERO, min_kyc: "B".into(), coins: vec![] };
        assert!(allowed(&s, &coin));
        s.coins = vec!["btc".into(), "usdt:trc20".into()];
        assert!(allowed(&s, &coin));
        s.coins = vec!["usdt:erc20".into()];
        assert!(!allowed(&s, &coin));
    }

    #[test]
    fn swap_amount_must_be_in_the_coin_range() {
        let mut coin = ExchangeCoin {
            ticker: "ltc".into(),
            network: "Mainnet".into(),
            name: "Litecoin".into(),
            memo: false,
            minimum: Some(Decimal::new(1, 1)),
            maximum: Some(Decimal::from(1000)),
        };
        assert!(check_limits(&coin, Decimal::ONE).is_ok());
        assert!(check_limits(&coin, Decimal::new(1, 1)).is_ok());
        assert!(check_limits(&coin, Decimal::ZERO).is_ok(), "no amount in the quote");
        let low = check_limits(&coin, Decimal::new(5, 2)).unwrap_err().to_string();
        assert!(low.contains("0.05 LTC") && low.contains("minimum of 0.1 LTC"), "{low}");
        assert!(check_limits(&coin, Decimal::from(1001)).unwrap_err().to_string().contains("maximum of 1000 LTC"));
        coin.maximum = Some(Decimal::ZERO);
        assert!(check_limits(&coin, Decimal::from(1_000_000)).is_ok(), "a zero maximum means no maximum");
    }

    #[test]
    fn every_payment_ticker_has_aliases() {
        for t in SETTLE_PREFERENCE {
            assert!(ALIASES.iter().any(|(a, _)| *a == t), "{t}");
        }
        assert_eq!(normalize_status("Paid partially"), "confirming");
        assert_eq!(normalize_status("whatever"), "waiting");
    }
}
