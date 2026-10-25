//! A local stand-in for the exchange API, for `--demo` and tests. It lists coins, quotes
//! with fixed prices and keeps swaps in memory. It never moves funds.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::error::{Error, Result};

/// Seconds a demo swap spends in each status before it moves on.
const DEMO_STEPS: [(u64, &str); 3] = [(40, "waiting"), (60, "confirming"), (75, "sending")];

struct Swap {
    body: Value,
    started: Instant,
    forced: Option<String>,
}

#[derive(Default)]
struct StubState {
    auto: bool,
    next_id: u64,
    swaps: HashMap<String, Swap>,
    requests: Vec<(String, HashMap<String, String>)>,
}

/// A running stub. Cheap to clone.
#[derive(Clone)]
pub struct Stub {
    pub base: String,
    state: Arc<Mutex<StubState>>,
}

const COINS: &[(&str, &str, &str, bool, f64)] = &[
    ("Bitcoin", "btc", "Mainnet", false, 65000.0),
    ("Monero", "xmr", "Mainnet", false, 160.0),
    ("Litecoin", "ltc", "Mainnet", false, 85.0),
    ("Ethereum", "eth", "ERC20", false, 3200.0),
    ("Ethereum", "eth", "Base", false, 3200.0),
    ("Tether", "usdt", "ERC20", false, 1.0),
    ("Tether", "usdt", "TRC20", false, 1.0),
    ("Tether", "usdt", "BEP20", false, 1.0),
    ("Tether", "usdt", "Polygon", false, 1.0),
    ("Tether", "usdt", "Solana", false, 1.0),
    ("USD Coin", "usdc", "ERC20", false, 1.0),
    ("USD Coin", "usdc", "Base", false, 1.0),
    ("USD Coin", "usdc", "BEP20", false, 1.0),
    ("USD Coin", "usdc", "Polygon", false, 1.0),
    ("USD Coin", "usdc", "Solana", false, 1.0),
    ("Solana", "sol", "Mainnet", false, 150.0),
    ("TRON", "trx", "Mainnet", false, 0.12),
    ("Toncoin", "ton", "Mainnet", true, 5.0),
    ("BNB", "bnb", "BEP20", false, 600.0),
    ("Polygon", "pol", "Polygon", false, 0.5),
    ("Dogecoin", "doge", "Mainnet", false, 0.15),
    ("Bitcoin Cash", "bch", "Mainnet", false, 420.0),
    ("Dash", "dash", "Mainnet", false, 30.0),
    ("Zcash", "zec", "Mainnet", false, 35.0),
    ("Cardano", "ada", "Mainnet", false, 0.45),
    ("Polkadot", "dot", "Mainnet", false, 6.5),
    ("Ripple", "xrp", "Mainnet", true, 0.55),
    ("Stellar", "xlm", "Mainnet", true, 0.11),
    ("Avalanche", "avax", "Mainnet", false, 30.0),
    ("Cosmos", "atom", "Mainnet", true, 8.0),
];

fn price(ticker: &str) -> f64 {
    COINS.iter().find(|c| c.1 == ticker).map(|c| c.4).unwrap_or(1.0)
}

fn dec(f: f64) -> Decimal {
    Decimal::try_from(f).unwrap_or_default().round_dp(8).normalize()
}

impl Stub {
    /// Start on a random local port. With `auto`, swaps move through their statuses with time.
    pub async fn start(auto: bool) -> Result<Stub> {
        let state = Arc::new(Mutex::new(StubState { auto, ..Default::default() }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.map_err(|e| Error::Internal(format!("exchange stub: {e}")))?;
        let addr = listener.local_addr().map_err(|e| Error::Internal(e.to_string()))?;
        let app = Router::new()
            .route("/coins", get(coins))
            .route("/new_rate", get(new_rate))
            .route("/new_trade", get(new_trade))
            .route("/trade", get(trade))
            .with_state(state.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Stub { base: format!("http://{addr}"), state })
    }

    /// Force a swap's status (tests).
    pub fn set_status(&self, trade_id: &str, status: &str) {
        if let Some(s) = self.lock().swaps.get_mut(trade_id) {
            s.forced = Some(status.to_string());
        }
    }

    /// Query parameters of the calls to `method`, oldest first.
    pub fn requests(&self, method: &str) -> Vec<HashMap<String, String>> {
        self.lock().requests.iter().filter(|(m, _)| m == method).map(|(_, q)| q.clone()).collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StubState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

type Shared = Arc<Mutex<StubState>>;

fn record(state: &Shared, method: &str, headers: &HeaderMap, q: &HashMap<String, String>) -> Option<Response> {
    let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
    s.requests.push((method.to_string(), q.clone()));
    let key = headers.get("api-key").and_then(|v| v.to_str().ok()).unwrap_or("");
    if key.is_empty() || key == "bad-key" {
        return Some((StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid API key" }))).into_response());
    }
    None
}

async fn coins(State(state): State<Shared>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    if let Some(r) = record(&state, "coins", &headers, &q) {
        return r;
    }
    let list: Vec<Value> = COINS
        .iter()
        .map(|(name, ticker, network, memo, p)| {
            json!({ "name": name, "ticker": ticker, "network": network, "memo": memo,
                    "image": "", "minimum": dec(10.0 / p), "maximum": dec(100_000.0 / p) })
        })
        .collect();
    Json(Value::Array(list)).into_response()
}

fn param<'a>(q: &'a HashMap<String, String>, k: &str) -> &'a str {
    q.get(k).map(String::as_str).unwrap_or("")
}

fn quote(q: &HashMap<String, String>) -> Option<(Decimal, Decimal)> {
    let to: f64 = param(q, "amount_to").parse().ok()?;
    let from = to * price(param(q, "ticker_to")) / price(param(q, "ticker_from")) * 1.012;
    Some((dec(from), dec(to)))
}

async fn new_rate(State(state): State<Shared>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    if let Some(r) = record(&state, "new_rate", &headers, &q) {
        return r;
    }
    if param(&q, "ticker_from") == "doge" || param(&q, "payment") != "True" {
        return Json(json!({ "error": "no rates for this pair" })).into_response();
    }
    let Some((from, to)) = quote(&q) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "amount_to is required" }))).into_response();
    };
    let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
    s.next_id += 1;
    let id = format!("R{:05}", s.next_id);
    Json(json!({
        "trade_id": id, "date": chrono::Utc::now().to_rfc3339(),
        "ticker_from": param(&q, "ticker_from"), "network_from": param(&q, "network_from"),
        "ticker_to": param(&q, "ticker_to"), "network_to": param(&q, "network_to"),
        "amount_from": from, "amount_to": to, "provider": "Quarry", "fixed": true, "status": "new", "payment": true,
        "quotes": { "quotes": [
            { "provider": "Quarry", "kycrating": "A", "amount_from": from.to_string(), "waste": "1.2" },
            { "provider": "Larkspur", "kycrating": "B", "amount_from": (from * Decimal::new(102, 2)).round_dp(8).to_string(), "waste": "3.1" }
        ], "expiresAt": (chrono::Utc::now() + chrono::Duration::minutes(30)).to_rfc3339() }
    }))
    .into_response()
}

async fn new_trade(State(state): State<Shared>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    if let Some(r) = record(&state, "new_trade", &headers, &q) {
        return r;
    }
    let missing = ["id", "address", "provider", "ticker_from", "network_from", "ticker_to", "network_to"]
        .into_iter()
        .find(|k| param(&q, k).is_empty());
    if let Some(k) = missing {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("{k} is required") }))).into_response();
    }
    let Some((from, to)) = quote(&q) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "amount_to is required" }))).into_response();
    };
    let ticker_from = param(&q, "ticker_from");
    let memo = COINS.iter().any(|c| c.1 == ticker_from && c.3);
    let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
    s.next_id += 1;
    let id = format!("T{:05}", s.next_id);
    let body = json!({
        "trade_id": id, "date": chrono::Utc::now().to_rfc3339(),
        "ticker_from": ticker_from, "network_from": param(&q, "network_from"),
        "ticker_to": param(&q, "ticker_to"), "network_to": param(&q, "network_to"),
        "coin_from": ticker_from, "coin_to": param(&q, "ticker_to"),
        "amount_from": from, "amount_to": to, "provider": param(&q, "provider"), "fixed": true, "status": "waiting",
        "address_provider": format!("{ticker_from}1demo{}q7x9k2m4p6r8t0v3w5y7z9a2c4e6g8j", s.next_id),
        "address_provider_memo": if memo { json!(format!("{}", 100_000 + s.next_id)) } else { json!("") },
        "address_user": param(&q, "address"), "address_user_memo": param(&q, "address_memo"),
        "refund_address": param(&q, "refund"), "refund_address_memo": param(&q, "refund_memo"),
        "password": "", "id_provider": format!("P{id}"), "payment": true,
        "quotes": { "support": {}, "expiresAt": (chrono::Utc::now() + chrono::Duration::minutes(30)).to_rfc3339() },
        "details": {}
    });
    s.swaps.insert(id, Swap { body: body.clone(), started: Instant::now(), forced: None });
    Json(body).into_response()
}

async fn trade(State(state): State<Shared>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    if let Some(r) = record(&state, "trade", &headers, &q) {
        return r;
    }
    let s = state.lock().unwrap_or_else(|e| e.into_inner());
    let Some(swap) = s.swaps.get(param(&q, "id")) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "trade not found" }))).into_response();
    };
    let status = match (&swap.forced, s.auto) {
        (Some(f), _) => f.clone(),
        (None, true) => {
            let secs = swap.started.elapsed().as_secs();
            DEMO_STEPS.iter().find(|(until, _)| secs < *until).map(|(_, st)| st.to_string()).unwrap_or_else(|| "finished".into())
        }
        (None, false) => "waiting".into(),
    };
    let mut body = swap.body.clone();
    if status == "finished" {
        body["details"] = json!({ "hashout": format!("demo-hashout-{}", param(&q, "id")) });
    }
    body["status"] = json!(status);
    Json(json!([body])).into_response()
}
