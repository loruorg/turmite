//! Split roles on one database: an API process that holds no key and a worker
//! process, with drivers built from the nodes stored in the database.
#![allow(dead_code)]

use std::{net::SocketAddr, sync::Arc, time::Duration};

use reqwest::Client;
use serde_json::Value;
use turmite::{
    App, Config, Options,
    config::Role,
    crypto::{HotIdentity, new_hot_identity},
    db::SweepRow,
    domain::Network,
    rates::FixedRateProvider,
};

use crate::common;

const PASSPHRASE: &str = "test-passphrase";

pub struct Split {
    pub api: App,
    pub worker: App,
    pub base: String,
    pub client: Client,
    pub db_url: String,
    identity_file: Vec<u8>,
}

async fn start_worker(db_url: &str, base: &str, identity_file: &[u8]) -> App {
    App::start(
        Config::for_tests(db_url, base),
        Options {
            role: Some(Role::Worker),
            rates: Some(Arc::new(FixedRateProvider::demo())),
            hot: Some(HotIdentity::from_encrypted(identity_file, PASSPHRASE).unwrap()),
            ..Default::default()
        },
    )
    .await
    .expect("start worker")
}

impl Split {
    /// A worker and an API on a fresh database, with one node for the network `code`.
    pub async fn start(code: &str, node_url: &str) -> Split {
        let db_url = common::fresh_database("turmite_chain").await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (recipient, identity_file) = new_hot_identity(PASSPHRASE).unwrap();
        // The worker starts first: it creates the deposit seed.
        let worker = start_worker(&db_url, &base, &identity_file).await;
        let n = network(&worker, code).await;
        let sealed = worker.sealer.seal_str("node_url", node_url);
        worker.store.add_node(n.network_id, "local", &sealed, node_url, 0).await.unwrap();
        worker.refresh_drivers().await.unwrap();
        assert!(worker.drivers().get(&n.chain).is_some_and(|d| d.can_derive()), "worker driver with the seed");

        let mut config = Config::for_tests(&db_url, &base);
        config.hot_recipient = Some(recipient);
        let api =
            App::start(config, Options { role: Some(Role::Api), rates: Some(Arc::new(FixedRateProvider::demo())), ..Default::default() })
                .await
                .expect("start api");
        assert!(api.drivers().get(&n.chain).is_some_and(|d| !d.can_derive()), "api driver without the seed");
        let router = turmite::http::router(api.clone());
        tokio::spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
        });
        let client = Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        Split { api, worker, base, client, db_url, identity_file }
    }

    /// Stop using the current worker and start a new one on the same database.
    pub async fn restart_worker(&mut self) {
        self.worker = start_worker(&self.db_url, &self.base, &self.identity_file).await;
    }

    pub async fn network(&self, code: &str) -> Network {
        network(&self.worker, code).await
    }

    /// A project that settles to `settle` on `code`, with an API key. `hot` = (family,
    /// address, secret) of a hot wallet for the project.
    pub async fn project(&self, code: &str, settle: &str, memo: Option<&str>, hot: Option<(&str, &str, &[u8])>) -> (i64, String) {
        let store = &self.worker.store;
        let pid = store.create_project("Chain shop", &self.worker.sealer.seal_str("ipn_secret", "secret")).await.unwrap();
        let n = self.network(code).await;
        store.upsert_project_wallet_address(pid, n.network_id, settle, memo).await.unwrap();
        if let Some((family, address, secret)) = hot {
            let id = turmite::hot::store(&self.api, family, "fees", address, secret, None).await.unwrap();
            turmite::hot::assign(&self.api, pid, family, id).await.unwrap();
        }
        let (key, _) = store.create_api_key(pid, "test", None).await.unwrap();
        (pid, key)
    }

    /// POST to the API. Returns (status, body, Retry-After).
    pub async fn post(&self, path: &str, key: &str, body: &Value) -> (u16, Value, Option<String>) {
        let r = self.client.post(format!("{}{path}", self.base)).header("x-api-key", key).json(body).send().await.unwrap();
        let status = r.status().as_u16();
        let retry = r.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
        (status, r.json().await.unwrap_or(Value::Null), retry)
    }

    pub async fn get(&self, path: &str, key: &str) -> Value {
        let r = self.client.get(format!("{}{path}", self.base)).header("x-api-key", key).send().await.unwrap();
        r.json().await.unwrap_or(Value::Null)
    }

    /// Run the worker on a payment until it is finished and swept, or `secs` pass.
    pub async fn settle(&self, key: &str, payment_id: i64, secs: u64) -> (Value, Vec<SweepRow>) {
        let end = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            if let Err(e) = turmite::worker::process_payment(&self.worker, payment_id).await {
                eprintln!("process_payment {payment_id}: {e}");
            }
            let p = self.get(&format!("/v1/payment/{payment_id}"), key).await;
            let sweeps = self.worker.store.sweeps_for_payment(payment_id).await.unwrap();
            if (p["payment_status"] == "finished" && !sweeps.is_empty()) || tokio::time::Instant::now() >= end {
                return (p, sweeps);
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    /// Check a payment's sweeps on chain until none is `broadcast`, or `secs` pass. Returns them.
    pub async fn sweeps_final(&self, payment_id: i64, secs: u64) -> Vec<SweepRow> {
        let end = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            turmite::worker::confirm_sweeps(&self.worker, payment_id).await.unwrap();
            let sweeps = self.worker.store.sweeps_for_payment(payment_id).await.unwrap();
            if sweeps.iter().all(|s| s.status != "broadcast") || tokio::time::Instant::now() >= end {
                return sweeps;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    /// Derivation index of a payment's deposit address.
    pub async fn deposit_index(&self, payment_id: i64) -> u64 {
        self.worker.store.deposit_address_for_payment(payment_id).await.unwrap().expect("deposit address").0 as u64
    }

    /// Point a catalog token at a test contract.
    pub async fn set_token_contract(&self, ticker: &str, contract: &str) {
        sqlx::query("UPDATE asset_network SET contract_address = $1 WHERE ticker = $2")
            .bind(contract)
            .bind(ticker)
            .execute(&self.worker.store.pool)
            .await
            .unwrap();
    }
}

async fn network(app: &App, code: &str) -> Network {
    app.store.list_networks().await.unwrap().into_iter().find(|n| n.code == code).expect("network")
}

/// 32 random bytes (a test key).
pub fn random_secret() -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(&hex::decode(turmite::crypto::random_hex(32)).unwrap());
    out
}
