//! Test harness: a fresh database and a running server per test.
//!
//! Needs PostgreSQL on 127.0.0.1:5433 (`docker compose -f deploy/testnets/docker-compose.yml up -d postgres`)
//! or `TURMITE_TEST_PG=postgres://user:pass@host:port`.
#![allow(dead_code)]

use std::{net::SocketAddr, sync::Arc};

use reqwest::Client;
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use turmite::{App, Config, Options, chains::DriverRegistry, crypto::HotIdentity, rates::FixedRateProvider};

pub fn pg_server() -> String {
    std::env::var("TURMITE_TEST_PG").unwrap_or_else(|_| "postgres://turmite:turmite@127.0.0.1:5433".into())
}

/// A new empty database. Returns its URL.
pub async fn fresh_database(prefix: &str) -> String {
    let server = pg_server();
    let name = format!("{prefix}_{}", uuid::Uuid::new_v4().simple());
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&format!("{server}/postgres"))
        .await
        .expect("connect to the test PostgreSQL (start deploy/testnets postgres)");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}"))).execute(&admin).await.unwrap();
    admin.close().await;
    format!("{server}/{name}")
}

pub struct TestApp {
    pub app: App,
    pub base: String,
    pub client: Client,
    pub db_url: String,
}

pub struct TestOptions {
    pub drivers: Option<DriverRegistry>,
    pub rates: FixedRateProvider,
    pub with_hot_identity: bool,
    pub demo: bool,
    /// Exchange API base (a stub).
    pub exchange_base: Option<String>,
}

impl Default for TestOptions {
    fn default() -> Self {
        Self {
            drivers: Some(DriverRegistry::all_mock()),
            rates: FixedRateProvider::demo(),
            with_hot_identity: true,
            demo: false,
            exchange_base: None,
        }
    }
}

/// Start a server on a random port with a fresh database.
pub async fn start(opts: TestOptions) -> TestApp {
    let db_url = fresh_database("turmite_test").await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");
    let config = Config::for_tests(&db_url, &base);
    let app = App::start(
        config,
        Options {
            role: None,
            demo: opts.demo,
            rates: Some(Arc::new(opts.rates)),
            drivers: opts.drivers,
            hot: opts.with_hot_identity.then(HotIdentity::generate),
            exchange_base: opts.exchange_base,
        },
    )
    .await
    .expect("start app");
    let router = turmite::http::router(app.clone());
    tokio::spawn(async move {
        axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    let client = Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    TestApp { app, base, client, db_url }
}

impl TestApp {
    /// A project with a wallet on every network and an API key. Returns (project_id, api_key).
    pub async fn project(&self) -> (i64, String) {
        let store = &self.app.store;
        let secret = self.app.sealer.seal_str("ipn_secret", "test-ipn-secret");
        let pid = store.create_project("Test shop", &secret).await.unwrap();
        for n in store.list_networks().await.unwrap() {
            store.upsert_project_wallet_address(pid, n.network_id, &format!("test-{}-wallet", n.chain), None).await.unwrap();
        }
        let (key, _) = store.create_api_key(pid, "test", None).await.unwrap();
        (pid, key)
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub async fn get(&self, path: &str, key: &str) -> (u16, Value) {
        let r = self.client.get(self.url(path)).header("x-api-key", key).send().await.unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn post(&self, path: &str, key: &str, body: &Value) -> (u16, Value) {
        let r = self.client.post(self.url(path)).header("x-api-key", key).json(body).send().await.unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }

    /// Stage a deposit on the mock driver and run the worker for the payment.
    pub async fn deposit(&self, payment_id: i64, amount: &str, confirmations: i32) {
        let p = self.app.store.payment_any(payment_id).await.unwrap().unwrap();
        let net = self.app.store.network(p.network_id).await.unwrap();
        let drivers = self.app.drivers();
        let driver = drivers.get(&net.chain).expect("driver");
        let mock = turmite::chains::driver::as_mock(&driver).expect("mock driver");
        let n = self.app.store.payment_transactions(payment_id).await.unwrap().len();
        let txid = format!("tx-{payment_id}-{n}");
        mock.inject(
            &p.pay_address,
            turmite::chains::driver::Observation {
                chain_tx_key: txid.clone(),
                txid,
                vout: None,
                amount: amount.parse().unwrap(),
                confirmations,
                block_height: None,
            },
        )
        .await;
        turmite::worker::process_payment(&self.app, payment_id).await.unwrap();
    }

    /// Run the worker once over every watched payment and deliver due webhooks.
    pub async fn tick(&self) {
        turmite::worker::tick(&self.app).await;
    }
}

/// Real-chain tests need the local nodes in deploy/testnets. They run only when
/// `TURMITE_TEST_CHAINS` lists their chain (`network.chain` values, comma-separated,
/// e.g. `bitcoin,monero`) or is `all`. CI does not set it, so these tests pass
/// without running. Start a real-chain test with:
/// `if !common::chain_enabled("bitcoin") { return; }`
pub fn chain_enabled(chain: &str) -> bool {
    let list = std::env::var("TURMITE_TEST_CHAINS").unwrap_or_default();
    let on = list.split([',', ' ']).map(str::trim).any(|c| c == "all" || c.eq_ignore_ascii_case(chain));
    if !on {
        eprintln!("SKIP: the {chain} test needs TURMITE_TEST_CHAINS={chain} (or all) and the deploy/testnets nodes");
    }
    on
}

/// `payment_id` of a payment object. Create responses send it as a string, status responses as a number.
pub fn pid(payment: &Value) -> i64 {
    match &payment["payment_id"] {
        Value::String(s) => s.parse().expect("numeric payment_id"),
        v => v.as_i64().expect("payment_id"),
    }
}
