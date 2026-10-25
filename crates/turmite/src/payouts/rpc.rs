//! Node access for payouts: the enabled nodes of a network, in priority order, and small HTTP helpers.

use std::time::Duration;

use serde_json::{Value, json};

use crate::{
    app::App,
    chains::registry::{node_url, redact_url},
    domain::Network,
    error::{Error, Result},
};

/// The nodes of one network.
#[derive(Clone)]
pub struct Nodes {
    pub network: Network,
    pub urls: Vec<String>,
    pub http: reqwest::Client,
}

impl Nodes {
    /// Enabled nodes of the network, decrypted. Errors when there is none.
    pub async fn load(app: &App, network: &Network) -> Result<Self> {
        let mut nodes = app.store.list_nodes(Some(network.network_id)).await?;
        nodes.retain(|n| n.is_enabled);
        // Tron, Solana and TON transactions do not name their network. Use only nodes that passed the
        // network check at connect. Tests and the demo set their drivers without nodes.
        if !app.drivers_fixed() {
            nodes.retain(|n| app.health.connected(network.network_id, n.node_id));
        }
        nodes.sort_by_key(|n| (n.priority, n.node_id));
        let urls: Vec<String> = nodes.iter().filter_map(|n| node_url(&app.sealer, n).ok()).collect();
        if urls.is_empty() {
            return Err(Error::Unavailable(format!(
                "{} has no connected node. Add one under Server > Networks, or see its error there.",
                network.name
            )));
        }
        Ok(Self { network: network.clone(), urls, http: app.http.clone() })
    }

    /// Run `f` on each node in order until one succeeds.
    pub async fn first_ok<T, F, Fut>(&self, f: F) -> Result<T>
    where
        F: Fn(String) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut last = None;
        for url in &self.urls {
            match f(url.clone()).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    tracing::debug!(node = %redact_url(url), error = %e, "payout node call failed");
                    last = Some(e);
                }
            }
        }
        Err(last.unwrap_or_else(|| Error::Driver("no node".into())))
    }

    /// JSON-RPC 2.0 call on the first node that answers. A JSON-RPC error from a node is returned as is.
    pub async fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        self.first_ok(|url| {
            let params = params.clone();
            async move { json_rpc(&self.http, &url, method, params).await }
        })
        .await
    }
}

/// One JSON-RPC 2.0 call. A node that does not answer gives `Error::Unavailable`; an error answer gives
/// `Error::Driver` with the node's message.
pub async fn json_rpc(http: &reqwest::Client, url: &str, method: &str, params: Value) -> Result<Value> {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let resp = http
        .post(url)
        .timeout(Duration::from_secs(15))
        .json(&body)
        .send()
        .await
        .map_err(|e| Error::Unavailable(format!("{method}: {}", e.without_url())))?;
    let v: Value = resp.json().await.map_err(|e| Error::Unavailable(format!("{method}: {}", e.without_url())))?;
    if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
        let msg = err.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| err.to_string());
        return Err(Error::Driver(format!("{method}: {msg}")));
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}

/// POST JSON to `base + path`. A node that does not answer gives `Error::Unavailable`.
pub async fn post_json(http: &reqwest::Client, base: &str, path: &str, body: &Value) -> Result<Value> {
    let url = format!("{}{path}", base.trim_end_matches('/'));
    let resp = http
        .post(&url)
        .timeout(Duration::from_secs(15))
        .json(body)
        .send()
        .await
        .map_err(|e| Error::Unavailable(format!("{path}: {}", e.without_url())))?;
    resp.json().await.map_err(|e| Error::Unavailable(format!("{path}: {}", e.without_url())))
}

/// GET `base + path` with a query. A node that does not answer gives `Error::Unavailable`.
pub async fn get_json(http: &reqwest::Client, base: &str, path: &str, query: &[(&str, &str)]) -> Result<Value> {
    let url = format!("{}{path}", base.trim_end_matches('/'));
    let resp = http
        .get(&url)
        .timeout(Duration::from_secs(15))
        .query(query)
        .send()
        .await
        .map_err(|e| Error::Unavailable(format!("{path}: {}", e.without_url())))?;
    resp.json().await.map_err(|e| Error::Unavailable(format!("{path}: {}", e.without_url())))
}

/// Parse a `0x` hex quantity.
pub fn hex_u128(v: &Value) -> Option<u128> {
    let s = v.as_str()?;
    u128::from_str_radix(s.trim_start_matches("0x"), 16).ok()
}

pub fn hex_u64(v: &Value) -> Option<u64> {
    hex_u128(v).and_then(|x| u64::try_from(x).ok())
}

/// A number given as a JSON number or a string.
pub fn as_u64(v: Option<&Value>) -> Option<u64> {
    match v? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}
