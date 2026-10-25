#![cfg(feature = "evm")]
//! EVM deposit contracts on the local anvil nodes of `deploy/testnets`: 127.0.0.1:8545 as Ethereum (chain id
//! 31337) and 127.0.0.1:8546 as BNB Smart Chain (chain id 97). A test skips when its node does not answer.
//!
//! Every test uses a new random cold wallet, so its factory and deposit addresses are new on the shared
//! nodes. The hot wallet is anvil account 0; customers and the test token use other accounts.

mod common;

use std::time::Duration;

use alloy::{primitives::Address, sol_types::SolConstructor};
use serde_json::{Value, json};
use sqlx::Row;
use turmite::{
    chains::{evm::EvmDriver, evm_factory},
    worker::evm_sweeper,
};

const ETH_RPC: &str = "http://127.0.0.1:8545";
const BSC_RPC: &str = "http://127.0.0.1:8546";
/// Anvil account 0 of the "test test … junk" mnemonic: the project's hot wallet.
const HOT: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";
const HOT_KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
/// Anvil account 1 pays; account 2 deploys and mints the test token.
const CUSTOMER: &str = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";
const MINTER: &str = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC";
const ETHER: u128 = 1_000_000_000_000_000_000;

// Creation code of a test ERC-20 with a public `mint` and `Transfer` events (solc 0.8.30, optimizer on).
// Constructor: (string name, string symbol, uint8 decimals). Source:
//   contract MockToken { string public name; string public symbol; uint8 public decimals;
//     mapping(address => uint256) public balanceOf; uint256 public totalSupply;
//     event Transfer(address indexed from, address indexed to, uint256 value);
//     constructor(string memory n, string memory s, uint8 d) { name = n; symbol = s; decimals = d; }
//     function mint(address to, uint256 a) external { balanceOf[to] += a; totalSupply += a; emit Transfer(address(0), to, a); }
//     function transfer(address to, uint256 a) external returns (bool) { require(balanceOf[msg.sender] >= a, "balance");
//       balanceOf[msg.sender] -= a; balanceOf[to] += a; emit Transfer(msg.sender, to, a); return true; } }
const MOCK_ERC20_CODE: &str = "0x608060405234801561000f575f5ffd5b5060405161070838038061070883398101604081905261002e91610100565b5f6100398482610201565b5060016100468382610201565b506002805460ff191660ff92909216919091179055506102bb9050565b634e487b7160e01b5f52604160045260245ffd5b5f82601f830112610086575f5ffd5b81516001600160401b0381111561009f5761009f610063565b604051601f8201601f19908116603f011681016001600160401b03811182821017156100cd576100cd610063565b6040528181528382016020018510156100e4575f5ffd5b8160208501602083015e5f918101602001919091529392505050565b5f5f5f60608486031215610112575f5ffd5b83516001600160401b03811115610127575f5ffd5b61013386828701610077565b602086015190945090506001600160401b03811115610150575f5ffd5b61015c86828701610077565b925050604084015160ff81168114610172575f5ffd5b809150509250925092565b600181811c9082168061019157607f821691505b6020821081036101af57634e487b7160e01b5f52602260045260245ffd5b50919050565b601f8211156101fc57805f5260205f20601f840160051c810160208510156101da5750805b601f840160051c820191505b818110156101f9575f81556001016101e6565b50505b505050565b81516001600160401b0381111561021a5761021a610063565b61022e81610228845461017d565b846101b5565b6020601f821160018114610260575f83156102495750848201515b5f19600385901b1c1916600184901b1784556101f9565b5f84815260208120601f198516915b8281101561028f578785015182556020948501946001909201910161026f565b50848210156102ac57868401515f19600387901b60f8161c191681555b50505050600190811b01905550565b610440806102c85f395ff3fe608060405234801561000f575f5ffd5b506004361061007a575f3560e01c806340c10f191161005857806340c10f19146100d257806370a08231146100e757806395d89b4114610106578063a9059cbb1461010e575f5ffd5b806306fdde031461007e57806318160ddd1461009c578063313ce567146100b3575b5f5ffd5b610086610131565b6040516100939190610336565b60405180910390f35b6100a560045481565b604051908152602001610093565b6002546100c09060ff1681565b60405160ff9091168152602001610093565b6100e56100e0366004610386565b6101bc565b005b6100a56100f53660046103ae565b60036020525f908152604090205481565b610086610244565b61012161011c366004610386565b610251565b6040519015158152602001610093565b5f805461013d906103ce565b80601f0160208091040260200160405190810160405280929190818152602001828054610169906103ce565b80156101b45780601f1061018b576101008083540402835291602001916101b4565b820191905f5260205f20905b81548152906001019060200180831161019757829003601f168201915b505050505081565b6001600160a01b0382165f90815260036020526040812080548392906101e390849061041a565b925050819055508060045f8282546101fb919061041a565b90915550506040518181526001600160a01b038316905f907fddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef9060200160405180910390a35050565b6001805461013d906103ce565b335f9081526003602052604081205482111561029d5760405162461bcd60e51b815260206004820152600760248201526662616c616e636560c81b604482015260640160405180910390fd5b335f90815260036020526040812080548492906102bb90849061042d565b90915550506001600160a01b0383165f90815260036020526040812080548492906102e790849061041a565b90915550506040518281526001600160a01b0384169033907fddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef9060200160405180910390a35060015b92915050565b602081525f82518060208401528060208501604085015e5f604082850101526040601f19601f83011684010191505092915050565b80356001600160a01b0381168114610381575f5ffd5b919050565b5f5f60408385031215610397575f5ffd5b6103a08361036b565b946020939093013593505050565b5f602082840312156103be575f5ffd5b6103c78261036b565b9392505050565b600181811c908216806103e257607f821691505b60208210810361040057634e487b7160e01b5f52602260045260245ffd5b50919050565b634e487b7160e01b5f52601160045260245ffd5b8082018082111561033057610330610406565b818103818111156103305761033061040656";

alloy::sol! {
    contract MockToken {
        constructor(string name, string symbol, uint8 decimals);
    }
}

async fn rpc(url: &str, method: &str, params: Value) -> Value {
    let body: Value = reqwest::Client::new()
        .post(url)
        .timeout(Duration::from_secs(30))
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(body.get("error").is_none(), "{method}: {body}");
    body["result"].clone()
}

async fn up(url: &str) -> bool {
    let r = reqwest::Client::new()
        .post(url)
        .timeout(Duration::from_secs(3))
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "eth_blockNumber", "params": [] }))
        .send()
        .await;
    r.is_ok()
}

fn hex_u(v: &Value) -> u128 {
    u128::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

/// Send from an unlocked anvil account and wait for a successful receipt.
async fn send(url: &str, from: &str, to: Option<&str>, value: u128, data: &str) -> Value {
    let mut tx = json!({ "from": from, "value": format!("{value:#x}"), "data": data });
    if let Some(to) = to {
        tx["to"] = json!(to);
    }
    let hash = rpc(url, "eth_sendTransaction", json!([tx])).await;
    for _ in 0..60 {
        let r = rpc(url, "eth_getTransactionReceipt", json!([hash])).await;
        if !r.is_null() {
            assert_eq!(r["status"], "0x1", "transaction failed: {r}");
            return r;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("no receipt for {hash}");
}

/// Mine blocks, then wait out the driver's one-second cache of the chain tip.
async fn mine(url: &str, blocks: u64) {
    rpc(url, "anvil_mine", json!([format!("{blocks:#x}")])).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
}

async fn balance(url: &str, address: &str) -> u128 {
    hex_u(&rpc(url, "eth_getBalance", json!([address, "latest"])).await)
}

async fn token_balance(url: &str, token: &str, owner: &str) -> u128 {
    let data = format!("0x70a08231{:0>64}", owner.trim_start_matches("0x").to_ascii_lowercase());
    hex_u(&rpc(url, "eth_call", json!([{ "to": token, "data": data }, "latest"])).await)
}

async fn has_code(url: &str, address: &str) -> bool {
    rpc(url, "eth_getCode", json!([address, "latest"])).await.as_str().unwrap().len() > 2
}

fn random_cold() -> Address {
    let b: [u8; 20] = rand_bytes();
    Address::from(b)
}

fn rand_bytes() -> [u8; 20] {
    let u = uuid::Uuid::new_v4();
    let v = uuid::Uuid::new_v4();
    let mut out = [0u8; 20];
    out[..16].copy_from_slice(u.as_bytes());
    out[16..].copy_from_slice(&v.as_bytes()[..4]);
    out
}

/// Deploy the test token and return its address.
async fn deploy_token(url: &str) -> String {
    let args = MockToken::constructorCall { name: "Mock USDC".into(), symbol: "USDC".into(), decimals: 6 }.abi_encode();
    let data = format!("{MOCK_ERC20_CODE}{}", alloy::primitives::hex::encode(args));
    let r = send(url, MINTER, None, 0, &data).await;
    r["contractAddress"].as_str().unwrap().to_string()
}

async fn network_id(t: &common::TestApp, chain: &str) -> i64 {
    t.app.store.network_by_chain(chain).await.unwrap().unwrap().network_id
}

/// Add a node for a network and rebuild the drivers.
async fn connect(t: &common::TestApp, chain: &str, url: &str) {
    let nid = network_id(t, chain).await;
    let sealed = t.app.sealer.seal_str("node_url", url);
    t.app.store.add_node(nid, "anvil", &sealed, url, 0).await.unwrap();
    t.app.refresh_drivers().await.unwrap();
}

/// A project with an API key and anvil account 0 as its EVM hot wallet. Returns (project_id, api_key).
async fn project(t: &common::TestApp) -> (i64, String) {
    let store = &t.app.store;
    let pid = store.create_project("EVM shop", &t.app.sealer.seal_str("ipn_secret", "s")).await.unwrap();
    let (key, _) = store.create_api_key(pid, "test", None).await.unwrap();
    let secret = hex::decode(HOT_KEY).unwrap();
    let hot = turmite::hot::store(&t.app, "evm", "anvil 0", HOT, &secret, None).await.unwrap();
    turmite::hot::assign(&t.app, pid, "evm", hot).await.unwrap();
    (pid, key)
}

/// A dashboard session cookie for a new root user (testnet: no two-factor code needed).
async fn login(t: &common::TestApp) -> String {
    let uid = t.app.store.create_user(&format!("{}@test.local", uuid::Uuid::new_v4().simple()), "x", "root").await.unwrap();
    let token = turmite::ids::token();
    let expires = chrono::Utc::now() + chrono::Duration::hours(1);
    t.app.store.create_session(&turmite::crypto::sha256_hex(token.as_bytes()), uid, None, None, expires).await.unwrap();
    format!("turmite_session={token}")
}

async fn form(t: &common::TestApp, cookie: &str, path: &str, fields: &[(&str, &str)]) -> reqwest::Response {
    t.client.post(t.url(path)).header("cookie", cookie).header("origin", &t.base).form(fields).send().await.unwrap()
}

async fn page(t: &common::TestApp, cookie: &str, path: &str) -> String {
    let r = t.client.get(t.url(path)).header("cookie", cookie).send().await.unwrap();
    assert_eq!(r.status(), 200);
    r.text().await.unwrap()
}

async fn create_payment(t: &common::TestApp, key: &str, usd: u32, currency: &str) -> Value {
    let (status, p) = t
        .post("/v1/payment", key, &json!({ "price_amount": usd, "price_currency": "usd", "pay_currency": currency, "order_id": "evm" }))
        .await;
    assert_eq!(status, 201, "{p}");
    p
}

async fn status(t: &common::TestApp, key: &str, id: i64) -> String {
    let (_, p) = t.get(&format!("/v1/payment/{id}"), key).await;
    p["payment_status"].as_str().unwrap().to_string()
}

/// Sweep now. Retries when a shared node rejected the nonce (other tests use account 0 too).
async fn sweep(t: &common::TestApp, pid: i64, nid: i64) -> Vec<String> {
    for _ in 0..4 {
        let sent = evm_sweeper::sweep_now(&t.app, pid, nid).await.unwrap();
        if !sent.is_empty() {
            return sent;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    panic!("no flush was sent");
}

/// Gas used by the hot-wallet transactions, oldest first: (kind, gas).
async fn gas_used(t: &common::TestApp) -> Vec<(String, i64)> {
    sqlx::query("SELECT kind, gas_used FROM evm_txs WHERE status = 'confirmed' AND gas_used IS NOT NULL ORDER BY evm_tx_id")
        .fetch_all(&t.app.store.pool)
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get("kind"), r.get("gas_used")))
        .collect()
}

fn wei(p: &Value, decimals: u32) -> u128 {
    let d: rust_decimal::Decimal = p["pay_amount"].to_string().parse().unwrap();
    (d * rust_decimal::Decimal::from(10u128.pow(decimals))).trunc().to_string().parse().unwrap()
}

#[tokio::test]
async fn eth_and_token_payments_are_swept_to_the_cold_wallet() {
    if !up(ETH_RPC).await {
        eprintln!("SKIP: anvil is not running on {ETH_RPC}");
        return;
    }
    let t = common::start(common::TestOptions { drivers: None, ..Default::default() }).await;
    let pool = &t.app.store.pool;
    sqlx::query("UPDATE network SET chain_id = 31337 WHERE chain = 'ethereum'").execute(pool).await.unwrap();
    let token = deploy_token(ETH_RPC).await;
    sqlx::query("UPDATE asset_network SET contract_address = $1 WHERE ticker = 'usdc'").bind(&token).execute(pool).await.unwrap();
    connect(&t, "ethereum", ETH_RPC).await;
    assert!(t.app.drivers().get("ethereum").is_some(), "driver connected");
    let eth = network_id(&t, "ethereum").await;
    let (pid, key) = project(&t).await;
    let cookie = login(&t).await;

    // The cold wallet: a wrong mixed-case checksum is refused, the EIP-55 form is kept.
    let cold = random_cold();
    let cold_s = cold.to_checksum(None);
    let bad = format!(
        "0x{}",
        cold_s[2..]
            .chars()
            .map(|c| if c.is_ascii_uppercase() { c.to_ascii_lowercase() } else { c.to_ascii_uppercase() })
            .collect::<String>()
    );
    let wallets = format!("/dashboard/p/{pid}/wallets");
    let r = form(&t, &cookie, &format!("{wallets}/evm"), &[("address", &bad), ("totp", "")]).await;
    assert_eq!(r.status(), 303);
    assert!(t.app.store.project_wallet(pid, eth).await.unwrap().is_none(), "a wrong checksum is refused");
    let r = form(&t, &cookie, &format!("{wallets}/evm"), &[("address", &cold_s.to_ascii_lowercase()), ("totp", "")]).await;
    assert_eq!(r.status(), 303);
    let factory = evm_factory::factory_address(cold);
    let implementation = evm_factory::implementation_address(factory);
    for chain in ["ethereum", "base", "polygon", "bsc"] {
        let w = t.app.store.project_wallet(pid, network_id(&t, chain).await).await.unwrap().unwrap();
        assert_eq!(w.address.as_deref(), Some(cold_s.as_str()), "{chain}");
        assert_eq!(w.factory_address, Some(factory.to_checksum(None)), "same factory on {chain}");
    }
    let html = page(&t, &cookie, &wallets).await;
    assert!(html.contains(&factory.to_checksum(None)), "factory shown");
    assert!(html.contains("Not deployed") && html.contains("Deploy now"));
    assert!(html.contains("Bitcoin") && html.contains("Monero"), "other families keep their cards");
    assert!(!has_code(ETH_RPC, &factory.to_string()).await);

    // Native payment: the deposit address is the factory's clone address for salt 1.
    let p1 = create_payment(&t, &key, 400, "eth").await;
    let id1 = common::pid(&p1);
    let addr1 = p1["pay_address"].as_str().unwrap().to_string();
    assert_eq!(addr1, evm_factory::predict_deposit_address(factory, implementation, evm_factory::salt_for_index(1)).to_checksum(None));
    let paid1 = wei(&p1, 18);
    assert_eq!(paid1, ETHER / 10);
    let tx1 = send(ETH_RPC, CUSTOMER, Some(&addr1), paid1, "0x").await;
    turmite::worker::process_payment(&t.app, id1).await.unwrap();
    let txs = t.app.store.payment_transactions(id1).await.unwrap();
    assert_eq!(txs.len(), 1);
    assert_eq!(txs[0].txid, tx1["transactionHash"].as_str().unwrap(), "native transfer found by its transaction");
    assert!(txs[0].confirmations >= 1);
    mine(ETH_RPC, 3).await;
    turmite::worker::process_payment(&t.app, id1).await.unwrap();
    assert_eq!(status(&t, &key, id1).await, "finished");

    // Token payment, plus some ether sent by mistake to the same address.
    let p2 = create_payment(&t, &key, 250, "usdc").await;
    let id2 = common::pid(&p2);
    let addr2 = p2["pay_address"].as_str().unwrap().to_string();
    let paid2 = wei(&p2, 6);
    assert_eq!(paid2, 250_000_000);
    let mint = format!("0x40c10f19{:0>64}{:064x}", addr2.trim_start_matches("0x").to_ascii_lowercase(), paid2);
    let tx2 = send(ETH_RPC, MINTER, Some(&token), 0, &mint).await;
    send(ETH_RPC, CUSTOMER, Some(&addr2), ETHER / 1000, "0x").await;
    mine(ETH_RPC, 3).await;
    turmite::worker::process_payment(&t.app, id2).await.unwrap();
    assert_eq!(status(&t, &key, id2).await, "finished");
    let txs = t.app.store.payment_transactions(id2).await.unwrap();
    assert_eq!(txs.len(), 1, "only the token counts for a token payment");
    assert_eq!(txs[0].txid, tx2["transactionHash"].as_str().unwrap());

    // One flush deploys the factory first, then empties both deposits into the cold wallet.
    let hot_before = balance(ETH_RPC, HOT).await;
    let sent = sweep(&t, pid, eth).await;
    assert_eq!(sent.len(), 1);
    assert!(has_code(ETH_RPC, &factory.to_string()).await, "factory deployed");
    assert_eq!(balance(ETH_RPC, &cold_s).await, paid1 + ETHER / 1000);
    assert_eq!(token_balance(ETH_RPC, &token, &cold_s).await, paid2);
    for a in [&addr1, &addr2] {
        assert_eq!(balance(ETH_RPC, a).await, 0);
        assert_eq!(token_balance(ETH_RPC, &token, a).await, 0);
    }
    assert!(balance(ETH_RPC, HOT).await < hot_before, "the hot wallet paid the gas");
    let rows =
        sqlx::query("SELECT payment_id, amount::text AS amount, status, txid FROM sweeps ORDER BY sweep_id").fetch_all(pool).await.unwrap();
    assert_eq!(rows.len(), 3, "two payments and the ether sent by mistake");
    for r in &rows {
        assert_eq!(r.get::<String, _>("status"), "confirmed");
        assert_eq!(r.get::<Option<String>, _>("txid").as_deref(), Some(sent[0].as_str()));
    }
    let amounts: Vec<(Option<i64>, rust_decimal::Decimal)> =
        rows.iter().map(|r| (r.get("payment_id"), r.get::<String, _>("amount").parse().unwrap())).collect();
    assert!(amounts.contains(&(Some(id1), "0.1".parse().unwrap())));
    assert!(amounts.contains(&(Some(id2), "250".parse().unwrap())));
    assert!(amounts.contains(&(None, "0.001".parse().unwrap())));
    let unswept: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM payments WHERE swept_gross < COALESCE(outcome_amount, 0)").fetch_one(pool).await.unwrap();
    assert_eq!(unswept, 0);

    // After the flush the native deposit still counts: balance plus the flushed amount.
    mine(ETH_RPC, 1).await;
    turmite::worker::process_payment(&t.app, id1).await.unwrap();
    let txs = t.app.store.payment_transactions(id1).await.unwrap();
    assert_eq!(txs.len(), 1);
    assert_eq!(txs[0].amount, "0.1".parse::<rust_decimal::Decimal>().unwrap());

    // A second native payment uses salt 3; the factory is there, so the sweep is one flush.
    let p3 = create_payment(&t, &key, 400, "eth").await;
    let id3 = common::pid(&p3);
    let addr3 = p3["pay_address"].as_str().unwrap().to_string();
    assert_eq!(addr3, evm_factory::predict_deposit_address(factory, implementation, evm_factory::salt_for_index(3)).to_checksum(None));
    send(ETH_RPC, CUSTOMER, Some(&addr3), wei(&p3, 18), "0x").await;
    mine(ETH_RPC, 3).await;
    turmite::worker::process_payment(&t.app, id3).await.unwrap();
    assert_eq!(status(&t, &key, id3).await, "finished");
    sweep(&t, pid, eth).await;
    assert_eq!(balance(ETH_RPC, &cold_s).await, 2 * paid1 + ETHER / 1000);
    assert_eq!(balance(ETH_RPC, &addr3).await, 0);

    // A late top-up to the same address is credited and flushed again (the clone exists now).
    send(ETH_RPC, CUSTOMER, Some(&addr3), ETHER / 50, "0x").await;
    mine(ETH_RPC, 3).await;
    turmite::worker::process_payment(&t.app, id3).await.unwrap();
    sweep(&t, pid, eth).await;
    assert_eq!(balance(ETH_RPC, &cold_s).await, 2 * paid1 + ETHER / 1000 + ETHER / 50);

    let gas = gas_used(&t).await;
    eprintln!("gas used on anvil (kind, gas): {gas:?}");
    let kinds: Vec<&str> = gas.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(kinds, ["deploy", "flush", "flush", "flush"], "one deployment only");

    let html = page(&t, &cookie, &wallets).await;
    assert!(html.contains("Deployed"));
}

#[tokio::test]
async fn bsc_deploy_now_then_native_sweep() {
    if !up(BSC_RPC).await {
        eprintln!("SKIP: anvil is not running on {BSC_RPC}");
        return;
    }
    let t = common::start(common::TestOptions { drivers: None, ..Default::default() }).await;
    connect(&t, "bsc", BSC_RPC).await;
    let bsc = network_id(&t, "bsc").await;
    let (pid, key) = project(&t).await;
    let cookie = login(&t).await;
    let cold = random_cold();
    let cold_s = cold.to_checksum(None);
    let factory = evm_factory::factory_address(cold);
    let wallets = format!("/dashboard/p/{pid}/wallets");
    assert_eq!(form(&t, &cookie, &format!("{wallets}/evm"), &[("address", &cold_s), ("totp", "")]).await.status(), 303);

    // "Deploy now" queues a deployment; the worker sends it with the hot wallet.
    assert_eq!(form(&t, &cookie, &format!("{wallets}/evm/{bsc}/deploy"), &[("totp", "")]).await.status(), 303);
    let queued: String = sqlx::query_scalar("SELECT status FROM evm_txs WHERE kind = 'deploy'").fetch_one(&t.app.store.pool).await.unwrap();
    assert_eq!(queued, "queued");
    assert!(page(&t, &cookie, &wallets).await.contains("Deploying"));
    for _ in 0..4 {
        evm_sweeper::pass(&t.app).await;
        if has_code(BSC_RPC, &factory.to_string()).await {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    assert!(has_code(BSC_RPC, &factory.to_string()).await, "factory deployed on BSC");
    assert!(page(&t, &cookie, &wallets).await.contains("Deployed"));

    let p = create_payment(&t, &key, 300, "bnbbsc").await;
    let id = common::pid(&p);
    let addr = p["pay_address"].as_str().unwrap().to_string();
    let paid = wei(&p, 18);
    assert_eq!(paid, ETHER * 3 / 10);
    send(BSC_RPC, CUSTOMER, Some(&addr), paid, "0x").await;
    mine(BSC_RPC, 3).await;
    turmite::worker::process_payment(&t.app, id).await.unwrap();
    assert_eq!(status(&t, &key, id).await, "finished");
    sweep(&t, pid, bsc).await;
    assert_eq!(balance(BSC_RPC, &cold_s).await, paid);
    assert_eq!(balance(BSC_RPC, &addr).await, 0);
    eprintln!("gas used on anvil-bsc (kind, gas): {:?}", gas_used(&t).await);
}

#[tokio::test]
async fn a_node_on_another_chain_is_refused() {
    if !up(ETH_RPC).await {
        eprintln!("SKIP: anvil is not running on {ETH_RPC}");
        return;
    }
    let t = common::start(common::TestOptions { drivers: None, ..Default::default() }).await;
    // The testnet catalog wants Sepolia (11155111); anvil is 31337.
    let network = t.app.store.network_by_chain("ethereum").await.unwrap().unwrap();
    let err = EvmDriver::connect(&network, ETH_RPC, t.app.store.clone()).await.err().expect("refused");
    assert!(err.to_string().contains("chain id 31337"), "{err}");
    connect(&t, "ethereum", ETH_RPC).await;
    assert!(t.app.drivers().get("ethereum").is_none());
    let nodes = t.app.store.list_nodes(Some(network.network_id)).await.unwrap();
    assert!(nodes[0].last_error.as_deref().unwrap_or_default().contains("chain id"), "{:?}", nodes[0].last_error);
}

#[tokio::test]
async fn a_queued_deployment_holds_back_the_sweep_deployment() {
    let t = common::start(Default::default()).await;
    let (pid, _) = t.project().await;
    let eth = t.app.store.network_by_chain("ethereum").await.unwrap().unwrap();
    let factory = "0x00000000000000000000000000000000000000aa";
    assert!(!t.app.store.evm_deploy_in_flight(eth.network_id, factory).await.unwrap());
    assert!(t.app.store.queue_evm_deploy(eth.network_id, pid, factory, HOT, None).await.unwrap());
    assert!(t.app.store.evm_deploy_in_flight(eth.network_id, factory).await.unwrap(), "queued counts as in flight");
}
