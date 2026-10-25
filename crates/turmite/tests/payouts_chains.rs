//! Payouts on the local chains of `deploy/testnets`: anvil (EVM native and ERC-20), solana-test-validator
//! (SOL and SPL), TRE (TRX and TRC-20) and MyLocalTon (TON). Run them with `TURMITE_TEST_CHAINS=all` (or a list of
//! chains); a test also skips when its node does not answer. The shared nodes are never reset: every run uses new
//! random keys.
#![cfg(feature = "all-chains")]

mod common;

use std::time::Duration;

use rust_decimal::Decimal;
use serde_json::{Value, json};
use turmite::{
    error::Error,
    hot,
    payouts::{
        PayoutStatus,
        limits::PayoutLimits,
        service::{self, NewWithdrawal, Source},
        signer,
    },
};

const ANVIL: &str = "http://127.0.0.1:8545";
const SOLANA: &str = "http://127.0.0.1:8899";

async fn rpc(url: &str, method: &str, params: Value) -> Value {
    let r = reqwest::Client::new()
        .post(url)
        .timeout(Duration::from_secs(20))
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
        .send()
        .await
        .expect("node");
    let v: Value = r.json().await.unwrap();
    assert!(v.get("error").is_none_or(Value::is_null), "{method}: {v}");
    v["result"].clone()
}

async fn up(url: &str, method: &str) -> bool {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": [] });
    reqwest::Client::new().post(url).timeout(Duration::from_secs(3)).json(&body).send().await.is_ok()
}

/// A project whose `family` hot wallet is a new key, with a node for `chain` and no approval needed.
async fn setup(t: &common::TestApp, family: &str, chain: &str, url: &str) -> (i64, i64, String) {
    let (pid, _) = t.project().await;
    let uid = t.app.store.create_user("ops@shop.test", "x", "member").await.unwrap();
    let secret = hot::generate_secret(family).unwrap();
    let address = hot::derive_address(family, &secret, true).unwrap();
    let id = hot::store(&t.app, family, "hot", &address, &secret, None).await.unwrap();
    hot::assign(&t.app, pid, family, id).await.unwrap();
    let net = t.app.store.network_by_chain(chain).await.unwrap().unwrap();
    t.app.store.add_node(net.network_id, "local", &t.app.sealer.seal_str("node_url", url), url, 0).await.unwrap();
    t.app.store.set_network_confirmations(net.network_id, 1).await.unwrap();
    let l = PayoutLimits { auto_approve_usd: Decimal::from(1_000_000_000), ..Default::default() };
    t.app.store.set_payout_limits(pid, &l).await.unwrap();
    (pid, uid, address)
}

async fn pay(t: &common::TestApp, pid: i64, uid: i64, currency: &str, to: &str, amount: &str) -> Result<i64, Error> {
    let w = NewWithdrawal { address: to.into(), currency: currency.into(), amount: amount.parse().unwrap(), ..Default::default() };
    let out = service::create_batch(&t.app, pid, Source::Dashboard { user_id: uid }, vec![w], None).await?;
    assert_eq!(out.payouts[0].status, PayoutStatus::Pending);
    Ok(out.payouts[0].payout_id)
}

async fn settle(t: &common::TestApp, id: i64) -> turmite::db::PayoutRow {
    signer::run_until_settled(&t.app, &[id], Duration::from_secs(90)).await.unwrap().remove(0)
}

// ---- EVM ---------------------------------------------------------------------------------

// Creation bytecode of a minimal ERC-20 with `constructor(string,string,uint8)`, public `mint`, `transfer`,
// `balanceOf` (solc 0.8.30).
const MOCK_ERC20_CODE: &str = "0x608060405234801561000f575f5ffd5b50604051610d0d380380610d0d833981810160405281019061003191906101f6565b8260029081610040919061048e565b508160039081610050919061048e565b508060015f6101000a81548160ff021916908360ff16021790555050505061055d565b5f604051905090565b5f5ffd5b5f5ffd5b5f5ffd5b5f5ffd5b5f601f19601f8301169050919050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52604160045260245ffd5b6100d28261008c565b810181811067ffffffffffffffff821117156100f1576100f061009c565b5b80604052505050565b5f610103610073565b905061010f82826100c9565b919050565b5f67ffffffffffffffff82111561012e5761012d61009c565b5b6101378261008c565b9050602081019050919050565b8281835e5f83830152505050565b5f61016461015f84610114565b6100fa565b9050828152602081018484840111156101805761017f610088565b5b61018b848285610144565b509392505050565b5f82601f8301126101a7576101a6610084565b5b81516101b7848260208601610152565b91505092915050565b5f60ff82169050919050565b6101d5816101c0565b81146101df575f5ffd5b50565b5f815190506101f0816101cc565b92915050565b5f5f5f6060848603121561020d5761020c61007c565b5b5f84015167ffffffffffffffff81111561022a57610229610080565b5b61023686828701610193565b935050602084015167ffffffffffffffff81111561025757610256610080565b5b61026386828701610193565b9250506040610274868287016101e2565b9150509250925092565b5f81519050919050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52602260045260245ffd5b5f60028204905060018216806102cc57607f821691505b6020821081036102df576102de610288565b5b50919050565b5f819050815f5260205f209050919050565b5f6020601f8301049050919050565b5f82821b905092915050565b5f600883026103417fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff82610306565b61034b8683610306565b95508019841693508086168417925050509392505050565b5f819050919050565b5f819050919050565b5f61038f61038a61038584610363565b61036c565b610363565b9050919050565b5f819050919050565b6103a883610375565b6103bc6103b482610396565b848454610312565b825550505050565b5f5f905090565b6103d36103c4565b6103de81848461039f565b505050565b5b81811015610401576103f65f826103cb565b6001810190506103e4565b5050565b601f82111561044657610417816102e5565b610420846102f7565b8101602085101561042f578190505b61044361043b856102f7565b8301826103e3565b50505b505050565b5f82821c905092915050565b5f6104665f198460080261044b565b1980831691505092915050565b5f61047e8383610457565b9150826002028217905092915050565b6104978261027e565b67ffffffffffffffff8111156104b0576104af61009c565b5b6104ba82546102b5565b6104c5828285610405565b5f60209050601f8311600181146104f6575f84156104e4578287015190505b6104ee8582610473565b865550610555565b601f198416610504866102e5565b5f5b8281101561052b57848901518255600182019150602085019450602081019050610506565b868310156105485784890151610544601f891682610457565b8355505b6001600288020188555050505b505050505050565b6107a38061056a5f395ff3fe608060405234801561000f575f5ffd5b5060043610610060575f3560e01c806306fdde0314610064578063313ce5671461008257806340c10f19146100a057806370a08231146100bc57806395d89b41146100ec578063a9059cbb1461010a575b5f5ffd5b61006c61013a565b604051610079919061046c565b60405180910390f35b61008a6101c6565b60405161009791906104a7565b60405180910390f35b6100ba60048036038101906100b59190610551565b6101d8565b005b6100d660048036038101906100d1919061058f565b61022e565b6040516100e391906105c9565b60405180910390f35b6100f4610242565b604051610101919061046c565b60405180910390f35b610124600480360381019061011f9190610551565b6102ce565b60405161013191906105fc565b60405180910390f35b6002805461014790610642565b80601f016020809104026020016040519081016040528092919081815260200182805461017390610642565b80156101be5780601f10610195576101008083540402835291602001916101be565b820191905f5260205f20905b8154815290600101906020018083116101a157829003601f168201915b505050505081565b60015f9054906101000a900460ff1681565b805f5f8473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020015f205f828254610223919061069f565b925050819055505050565b5f602052805f5260405f205f915090505481565b6003805461024f90610642565b80601f016020809104026020016040519081016040528092919081815260200182805461027b90610642565b80156102c65780601f1061029d576101008083540402835291602001916102c6565b820191905f5260205f20905b8154815290600101906020018083116102a957829003601f168201915b505050505081565b5f815f5f3373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020015f2054101561034e576040517f08c379a00000000000000000000000000000000000000000000000000000000081526004016103459061071c565b60405180910390fd5b815f5f3373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020015f205f828254610399919061073a565b92505081905550815f5f8573ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020015f205f8282546103eb919061069f565b925050819055506001905092915050565b5f81519050919050565b5f82825260208201905092915050565b8281835e5f83830152505050565b5f601f19601f8301169050919050565b5f61043e826103fc565b6104488185610406565b9350610458818560208601610416565b61046181610424565b840191505092915050565b5f6020820190508181035f8301526104848184610434565b905092915050565b5f60ff82169050919050565b6104a18161048c565b82525050565b5f6020820190506104ba5f830184610498565b92915050565b5f5ffd5b5f73ffffffffffffffffffffffffffffffffffffffff82169050919050565b5f6104ed826104c4565b9050919050565b6104fd816104e3565b8114610507575f5ffd5b50565b5f81359050610518816104f4565b92915050565b5f819050919050565b6105308161051e565b811461053a575f5ffd5b50565b5f8135905061054b81610527565b92915050565b5f5f60408385031215610567576105666104c0565b5b5f6105748582860161050a565b92505060206105858582860161053d565b9150509250929050565b5f602082840312156105a4576105a36104c0565b5b5f6105b18482850161050a565b91505092915050565b6105c38161051e565b82525050565b5f6020820190506105dc5f8301846105ba565b92915050565b5f8115159050919050565b6105f6816105e2565b82525050565b5f60208201905061060f5f8301846105ed565b92915050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52602260045260245ffd5b5f600282049050600182168061065957607f821691505b60208210810361066c5761066b610615565b5b50919050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52601160045260245ffd5b5f6106a98261051e565b91506106b48361051e565b92508282019050808211156106cc576106cb610672565b5b92915050565b7f62616c00000000000000000000000000000000000000000000000000000000005f82015250565b5f610706600383610406565b9150610711826106d2565b602082019050919050565b5f6020820190508181035f830152610733816106fa565b9050919050565b5f6107448261051e565b915061074f8361051e565b925082820390508181111561076757610766610672565b5b9291505056fea26469706673582212206a69db8b364efdeb4e931f6895c240a0de768ff32034997f9a77e8b3effb988364736f6c634300081e0033";

alloy::sol! {
    #[allow(missing_docs)]
    contract MockErc20 {
        constructor(string name, string symbol, uint8 decimals);
        function mint(address to, uint256 amount);
        function balanceOf(address owner) external view returns (uint256);
    }
}

/// Sign and send a transaction from `key` with anvil's chain id; wait for its receipt.
async fn evm_send(key: &alloy::signers::local::PrivateKeySigner, to: alloy::primitives::TxKind, input: Vec<u8>) -> Value {
    use alloy::consensus::{SignableTransaction, TxEip1559, TxEnvelope};
    use alloy::eips::eip2718::Encodable2718;
    use alloy::signers::SignerSync;
    let from = key.address().to_checksum(None);
    let nonce = u64::from_str_radix(
        rpc(ANVIL, "eth_getTransactionCount", json!([from, "pending"])).await.as_str().unwrap().trim_start_matches("0x"),
        16,
    )
    .unwrap();
    let tx = TxEip1559 {
        chain_id: 31337,
        nonce,
        gas_limit: 3_000_000,
        max_fee_per_gas: 50_000_000_000,
        max_priority_fee_per_gas: 1_000_000_000,
        to,
        value: Default::default(),
        access_list: Default::default(),
        input: input.into(),
    };
    let sig = key.sign_hash_sync(&tx.signature_hash()).unwrap();
    let raw = TxEnvelope::Eip1559(tx.into_signed(sig)).encoded_2718();
    let hash = rpc(ANVIL, "eth_sendRawTransaction", json!([format!("0x{}", hex::encode(raw))])).await;
    for _ in 0..50 {
        let r = rpc(ANVIL, "eth_getTransactionReceipt", json!([hash])).await;
        if !r.is_null() {
            assert_eq!(r["status"], "0x1", "{r}");
            return r;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("no receipt for {hash}");
}

async fn evm_balance(address: &str) -> u128 {
    u128::from_str_radix(rpc(ANVIL, "eth_getBalance", json!([address, "latest"])).await.as_str().unwrap().trim_start_matches("0x"), 16)
        .unwrap()
}

async fn erc20_balance(token: &str, owner: alloy::primitives::Address) -> u128 {
    use alloy::sol_types::SolCall;
    let data = MockErc20::balanceOfCall { owner }.abi_encode();
    let v = rpc(ANVIL, "eth_call", json!([{ "to": token, "data": format!("0x{}", hex::encode(data)) }, "latest"])).await;
    u128::from_str_radix(&v.as_str().unwrap()[2..][32..], 16).unwrap()
}

fn random_evm() -> alloy::signers::local::PrivateKeySigner {
    alloy::signers::local::PrivateKeySigner::from_slice(&hex::decode(turmite::crypto::random_hex(32)).unwrap()).unwrap()
}

#[tokio::test]
async fn evm_native_and_erc20_payouts_on_anvil() {
    if !common::chain_enabled("ethereum") {
        return;
    }
    use alloy::primitives::{TxKind, U256};
    use alloy::sol_types::{SolCall, SolConstructor};
    if !up(ANVIL, "eth_chainId").await {
        eprintln!("SKIP: anvil is not running on {ANVIL}");
        return;
    }
    let t = common::start(Default::default()).await;
    sqlx::query("UPDATE network SET chain_id = 31337 WHERE code = 'ETH'").execute(&t.app.store.pool).await.unwrap();
    let (pid, uid, hot_address) = setup(&t, "evm", "ethereum", ANVIL).await;
    // Fund the hot wallet without a transaction (no nonce race with other users of this anvil).
    rpc(ANVIL, "anvil_setBalance", json!([hot_address, "0x8ac7230489e80000"])).await; // 10 ETH

    // Native: 0.5 ETH to a new address.
    let to = random_evm().address();
    let id = pay(&t, pid, uid, "eth", &to.to_checksum(None), "0.5").await.unwrap();
    let p = settle(&t, id).await;
    assert_eq!(p.status, PayoutStatus::Sent, "{:?}", p.error);
    assert_eq!(evm_balance(&to.to_checksum(None)).await, 500_000_000_000_000_000);
    assert!(p.fee.unwrap() > Decimal::ZERO);
    assert_eq!(p.txid.as_deref().map(str::len), Some(66));
    let attempts = t.app.store.payout_attempts(id).await.unwrap();
    assert_eq!(attempts.len(), 1);
    assert!(attempts[0].raw_tx.starts_with("0x02"), "the signed transaction was stored");
    assert!(attempts[0].broadcast_at.is_some());

    // ERC-20: deploy a 6-decimal token, mint to the hot wallet, point `usdc` at it.
    let deployer = random_evm();
    rpc(ANVIL, "anvil_setBalance", json!([deployer.address().to_checksum(None), "0x8ac7230489e80000"])).await;
    let mut code = hex::decode(MOCK_ERC20_CODE.trim_start_matches("0x")).unwrap();
    code.extend(MockErc20::constructorCall { name: "Mock USDC".into(), symbol: "USDC".into(), decimals: 6 }.abi_encode());
    let receipt = evm_send(&deployer, TxKind::Create, code).await;
    let token = receipt["contractAddress"].as_str().unwrap().to_string();
    let hot: alloy::primitives::Address = hot_address.parse().unwrap();
    evm_send(
        &deployer,
        TxKind::Call(token.parse().unwrap()),
        MockErc20::mintCall { to: hot, amount: U256::from(100_000_000u64) }.abi_encode(),
    )
    .await;
    let eth = t.app.store.network_by_chain("ethereum").await.unwrap().unwrap();
    t.app.store.upsert_token(eth.network_id, "USDC", "usdc", 6, &token, None).await.unwrap();

    let to = random_evm().address();
    let id = pay(&t, pid, uid, "usdc", &to.to_checksum(None), "12.5").await.unwrap();
    let p = settle(&t, id).await;
    assert_eq!(p.status, PayoutStatus::Sent, "{:?}", p.error);
    assert_eq!(erc20_balance(&token, to).await, 12_500_000);
    assert_eq!(erc20_balance(&token, hot).await, 87_500_000);

    // More than the hot wallet holds: the payout fails with a clear message, nothing is signed.
    let id = pay(&t, pid, uid, "usdc", &to.to_checksum(None), "1000").await.unwrap();
    let p = settle(&t, id).await;
    assert_eq!(p.status, PayoutStatus::Failed);
    assert!(p.error.as_deref().unwrap_or("").contains("hot wallet holds"), "{:?}", p.error);
    assert!(t.app.store.payout_attempts(id).await.unwrap().is_empty());

    // Two payouts at once from the same wallet get consecutive nonces.
    let a = pay(&t, pid, uid, "eth", &random_evm().address().to_checksum(None), "0.01").await.unwrap();
    let b = pay(&t, pid, uid, "eth", &random_evm().address().to_checksum(None), "0.02").await.unwrap();
    let rows = signer::run_until_settled(&t.app, &[a, b], Duration::from_secs(90)).await.unwrap();
    assert!(rows.iter().all(|p| p.status == PayoutStatus::Sent), "{:?}", rows.iter().map(|p| (&p.status, &p.error)).collect::<Vec<_>>());
    let na = t.app.store.payout_attempts(a).await.unwrap()[0].nonce.unwrap();
    let nb = t.app.store.payout_attempts(b).await.unwrap()[0].nonce.unwrap();
    assert_eq!(nb, na + 1);

    // The fee estimate comes from the chain.
    let (fee, ticker, in_token) = service::estimate_fee(&t.app, pid, "usdc").await.unwrap();
    assert_eq!(ticker, "eth");
    assert!(fee > Decimal::ZERO && in_token > Decimal::ZERO);
    let (key, _) = t.app.store.create_api_key(pid, "fee", None).await.unwrap();
    let (status, v) = t.get("/v1/payout/fee?currency=eth&amount=0.5", &key).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!((v["currency"].as_str(), v["fee_currency"].as_str(), v["amount"].as_str()), (Some("eth"), Some("eth"), Some("0.5")));
    assert!(v["fee"].as_f64().unwrap() > 0.0);
}

// ---- Solana ------------------------------------------------------------------------------

mod sol {
    use super::*;
    use base64::Engine;
    use ed25519_dalek::SigningKey;
    use turmite::payouts::sol::{
        AccountMeta, Instruction, SYSTEM_PROGRAM, TOKEN_PROGRAM, associated_token_address, compile_message, create_ata_idempotent,
        decode_pubkey, encode_pubkey, sign_transaction,
    };

    pub fn random_key() -> SigningKey {
        let bytes: [u8; 32] = hex::decode(turmite::crypto::random_hex(32)).unwrap().try_into().unwrap();
        SigningKey::from_bytes(&bytes)
    }

    pub fn pubkey(k: &SigningKey) -> String {
        encode_pubkey(&k.verifying_key().to_bytes())
    }

    pub async fn balance(address: &str) -> u64 {
        rpc(SOLANA, "getBalance", json!([address, { "commitment": "confirmed" }])).await["value"].as_u64().unwrap()
    }

    pub async fn airdrop(address: &str, lamports: u64) {
        let before = balance(address).await;
        rpc(SOLANA, "requestAirdrop", json!([address, lamports])).await;
        for _ in 0..60 {
            if balance(address).await >= before + lamports {
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!("airdrop to {address} did not land");
    }

    pub async fn send(ixs: &[Instruction], signers: &[&SigningKey]) {
        let payer = signers[0].verifying_key().to_bytes();
        let bh = rpc(SOLANA, "getLatestBlockhash", json!([{ "commitment": "confirmed" }])).await;
        let blockhash: [u8; 32] = bs58::decode(bh["value"]["blockhash"].as_str().unwrap()).into_vec().unwrap().try_into().unwrap();
        let (message, _) = compile_message(&payer, ixs, &blockhash);
        let (tx, sig) = sign_transaction(&message, signers);
        rpc(
            SOLANA,
            "sendTransaction",
            json!([base64::engine::general_purpose::STANDARD.encode(tx), { "encoding": "base64", "preflightCommitment": "confirmed" }]),
        )
        .await;
        for _ in 0..60 {
            let st = rpc(SOLANA, "getSignatureStatuses", json!([[sig]])).await;
            let s = &st["value"][0];
            if !s.is_null() && s["confirmationStatus"] != "processed" {
                assert!(s["err"].is_null(), "{s}");
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!("transaction {sig} did not confirm");
    }

    /// A new 6-decimal mint with `authority` as mint authority, and `amount` minted to the authority's
    /// associated token account.
    pub async fn new_mint(authority: &SigningKey, amount: u64) -> String {
        let mint = random_key();
        let mint_pk = mint.verifying_key().to_bytes();
        let auth_pk = authority.verifying_key().to_bytes();
        let token_program = decode_pubkey(TOKEN_PROGRAM).unwrap();
        let rent = rpc(SOLANA, "getMinimumBalanceForRentExemption", json!([82])).await.as_u64().unwrap();
        let mut create = vec![0, 0, 0, 0];
        create.extend_from_slice(&rent.to_le_bytes());
        create.extend_from_slice(&82u64.to_le_bytes());
        create.extend_from_slice(&token_program);
        let create = Instruction {
            program_id: SYSTEM_PROGRAM,
            accounts: vec![AccountMeta::new(auth_pk, true, true), AccountMeta::new(mint_pk, true, true)],
            data: create,
        };
        let mut init = vec![20, 6];
        init.extend_from_slice(&auth_pk);
        init.push(0);
        let init = Instruction { program_id: token_program, accounts: vec![AccountMeta::new(mint_pk, false, true)], data: init };
        send(&[create, init], &[authority, &mint]).await;
        let ata = associated_token_address(&auth_pk, &mint_pk, &token_program);
        let mut mint_to = vec![7];
        mint_to.extend_from_slice(&amount.to_le_bytes());
        let mint_to = Instruction {
            program_id: token_program,
            accounts: vec![
                AccountMeta::new(mint_pk, false, true),
                AccountMeta::new(ata, false, true),
                AccountMeta::new(auth_pk, true, false),
            ],
            data: mint_to,
        };
        send(&[create_ata_idempotent(&auth_pk, &ata, &auth_pk, &mint_pk, &token_program), mint_to], &[authority]).await;
        encode_pubkey(&mint_pk)
    }

    pub async fn token_balance(owner: &str, mint: &str) -> u64 {
        let ata =
            associated_token_address(&decode_pubkey(owner).unwrap(), &decode_pubkey(mint).unwrap(), &decode_pubkey(TOKEN_PROGRAM).unwrap());
        rpc(SOLANA, "getTokenAccountBalance", json!([encode_pubkey(&ata), { "commitment": "confirmed" }])).await["value"]["amount"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    }

    pub fn ata_of(owner: &str, mint: &str) -> String {
        encode_pubkey(&associated_token_address(
            &decode_pubkey(owner).unwrap(),
            &decode_pubkey(mint).unwrap(),
            &decode_pubkey(TOKEN_PROGRAM).unwrap(),
        ))
    }
}

#[tokio::test]
async fn solana_sol_and_spl_payouts_on_the_test_validator() {
    if !common::chain_enabled("solana") {
        return;
    }
    if !up(SOLANA, "getHealth").await {
        eprintln!("SKIP: solana-test-validator is not running on {SOLANA}");
        return;
    }
    let t = common::start(Default::default()).await;
    let (pid, uid, hot_address) = setup(&t, "solana", "solana", SOLANA).await;
    sol::airdrop(&hot_address, 5_000_000_000).await;

    // SOL to a new account.
    let to = sol::pubkey(&sol::random_key());
    let id = pay(&t, pid, uid, "sol", &to, "0.01").await.unwrap();
    let p = settle(&t, id).await;
    assert_eq!(p.status, PayoutStatus::Sent, "{:?}", p.error);
    assert_eq!(sol::balance(&to).await, 10_000_000);
    assert_eq!(p.fee, Some("0.000005".parse().unwrap()));

    // Below the rent-exempt minimum of a new account: refused before it is made.
    let e = pay(&t, pid, uid, "sol", &sol::pubkey(&sol::random_key()), "0.0001").await.unwrap_err();
    assert!(e.to_string().contains("rent-exempt"), "{e}");
    // An existing account can take a small amount.
    let id = pay(&t, pid, uid, "sol", &to, "0.0001").await.unwrap();
    assert_eq!(settle(&t, id).await.status, PayoutStatus::Sent);

    // SPL: a new mint; the hot wallet pays to create the recipient's token account.
    let hot_key = {
        let k = hot::key_for_project(&t.app, pid, "solana").await.unwrap().unwrap();
        ed25519_dalek::SigningKey::from_bytes(&k.secret.as_slice().try_into().unwrap())
    };
    let mint = sol::new_mint(&hot_key, 100_000_000).await;
    let net = t.app.store.network_by_chain("solana").await.unwrap().unwrap();
    t.app.store.upsert_token(net.network_id, "USDC", "usdcsol", 6, &mint, None).await.unwrap();
    let to = sol::pubkey(&sol::random_key());
    let id = pay(&t, pid, uid, "usdcsol", &to, "2.5").await.unwrap();
    let p = settle(&t, id).await;
    assert_eq!(p.status, PayoutStatus::Sent, "{:?}", p.error);
    assert_eq!(sol::token_balance(&to, &mint).await, 2_500_000);
    // A second payout reuses the account.
    let id = pay(&t, pid, uid, "usdcsol", &to, "1").await.unwrap();
    assert_eq!(settle(&t, id).await.status, PayoutStatus::Sent);
    assert_eq!(sol::token_balance(&to, &mint).await, 3_500_000);

    // A token account is not a valid recipient.
    let e = pay(&t, pid, uid, "usdcsol", &sol::ata_of(&to, &mint), "1").await.unwrap_err();
    assert!(e.to_string().contains("token account"), "{e}");
    let e = pay(&t, pid, uid, "sol", &sol::ata_of(&to, &mint), "0.01").await.unwrap_err();
    assert!(e.to_string().contains("token account"), "{e}");
}

// ---- Tron ----------------------------------------------------------------------------------

const TRE: &str = "http://127.0.0.1:9090";

async fn tron_post(path: &str, body: Value) -> Value {
    reqwest::Client::new()
        .post(format!("{TRE}{path}"))
        .timeout(Duration::from_secs(20))
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// The first prefunded account of TRE, from its log.
fn tre_funder() -> Option<String> {
    let out = std::process::Command::new("docker").args(["logs", "turmite-tron"]).output().ok()?;
    let log = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    let keys = log.split("Private Keys").nth(1)?;
    keys.lines().find_map(|l| l.trim().strip_prefix("(0) ").map(str::to_string))
}

async fn tron_balance(address: &str) -> u64 {
    tron_post("/wallet/getaccount", json!({ "address": address, "visible": true })).await["balance"].as_u64().unwrap_or(0)
}

/// Send TRX from a private key with the payout module's own encoder; wait until it is in a block.
async fn tron_send(secret_hex: &str, to: &str, sun: i64) {
    use turmite::payouts::tron::{Call, RefBlock, address_from_secret, decode_address, raw_data, sign_raw};
    let secret = hex::decode(secret_hex).unwrap();
    let owner = decode_address(&address_from_secret(&secret).unwrap()).unwrap();
    let to = decode_address(to).unwrap();
    let b = tron_post("/wallet/getnowblock", json!({})).await;
    let block = RefBlock {
        number: b["block_header"]["raw_data"]["number"].as_u64().unwrap(),
        id: hex::decode(b["blockID"].as_str().unwrap()).unwrap().try_into().unwrap(),
        timestamp_ms: b["block_header"]["raw_data"]["timestamp"].as_i64().unwrap(),
    };
    let raw = raw_data(
        &Call::Trx { owner: &owner, to: &to, amount: sun },
        &block,
        block.timestamp_ms.max(chrono::Utc::now().timestamp_millis()) + 60_000,
        None,
    );
    let (txid, tx) = sign_raw(&k256::ecdsa::SigningKey::from_slice(&secret).unwrap(), &raw).unwrap();
    let r = tron_post("/wallet/broadcasthex", json!({ "transaction": hex::encode(tx) })).await;
    assert_eq!(r["result"], true, "{r}");
    for _ in 0..60 {
        if tron_post("/wallet/gettransactioninfobyid", json!({ "value": txid })).await.get("blockNumber").is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("funding transaction {txid} is not in a block");
}

#[tokio::test]
async fn tron_trx_payout_on_tre() {
    if !common::chain_enabled("tron") {
        return;
    }
    if tron_post_ok().await.is_none() {
        eprintln!("SKIP: TRE is not running on {TRE}");
        return;
    }
    let Some(funder) = tre_funder() else {
        eprintln!("SKIP: no TRE funder key in the container log");
        return;
    };
    let t = common::start(Default::default()).await;
    let (pid, uid, hot_address) = setup(&t, "tron", "tron", TRE).await;
    tron_send(&funder, &hot_address, 1_000_000_000).await; // 1000 TRX

    let to_secret = hot::generate_secret("tron").unwrap();
    let to = hot::derive_address("tron", &to_secret, true).unwrap();
    let id = pay(&t, pid, uid, "trx", &to, "12.5").await.unwrap();
    let p = settle(&t, id).await;
    assert_eq!(p.status, PayoutStatus::Sent, "{:?}", p.error);
    assert_eq!(tron_balance(&to).await, 12_500_000);
    assert_eq!(p.txid.as_deref().map(str::len), Some(64));
    // Too much: refused before signing.
    let id = pay(&t, pid, uid, "trx", &to, "5000").await.unwrap();
    let p = settle(&t, id).await;
    assert_eq!(p.status, PayoutStatus::Failed);
    assert!(p.error.as_deref().unwrap_or("").contains("hot wallet holds"), "{:?}", p.error);

    // TRC-20: deploy the 6-decimal test token, mint to the hot wallet, point `usdttrc20` at it.
    let token = tron_deploy_token(&funder).await;
    tron_call(&funder, &token, "mint(address,uint256)", &format!("{}{:064x}", tron_word_address(&hot_address), 50_000_000u64)).await;
    let net = t.app.store.network_by_chain("tron").await.unwrap().unwrap();
    t.app.store.upsert_token(net.network_id, "USDT", "usdttrc20", 6, &token, None).await.unwrap();
    let id = pay(&t, pid, uid, "usdttrc20", &to, "7.5").await.unwrap();
    let p = settle(&t, id).await;
    assert_eq!(p.status, PayoutStatus::Sent, "{:?}", p.error);
    assert_eq!(tron_token_balance(&token, &to).await, 7_500_000);
    assert_eq!(tron_token_balance(&token, &hot_address).await, 42_500_000);
    assert!(p.fee.unwrap() > Decimal::ZERO, "energy was paid in TRX");
}

fn tron_b58(bytes: &[u8]) -> String {
    use sha2::Digest;
    let check = sha2::Sha256::digest(sha2::Sha256::digest(bytes));
    let mut full = bytes.to_vec();
    full.extend_from_slice(&check[..4]);
    bs58::encode(full).into_string()
}

/// A Tron address as a 32-byte ABI word (hex).
fn tron_word_address(address: &str) -> String {
    format!("{}{}", "0".repeat(24), hex::encode(&turmite::payouts::tron::decode_address(address).unwrap()[1..]))
}

/// Sign a node-built transaction (`txID`) and broadcast it; wait for its block. Returns the transaction info.
async fn tron_sign_broadcast(secret_hex: &str, mut tx: Value) -> Value {
    let secret = hex::decode(secret_hex).unwrap();
    let sk = k256::ecdsa::SigningKey::from_slice(&secret).unwrap();
    let txid = tx["txID"].as_str().unwrap_or_else(|| panic!("no txID: {tx}")).to_string();
    let (sig, recid) = sk.sign_prehash_recoverable(&hex::decode(&txid).unwrap());
    let mut sig = sig.to_bytes().to_vec();
    sig.push(recid.to_byte());
    tx["signature"] = json!([hex::encode(sig)]);
    let r = tron_post("/wallet/broadcasttransaction", tx).await;
    assert_eq!(r["result"], true, "{r}");
    for _ in 0..60 {
        let info = tron_post("/wallet/gettransactioninfobyid", json!({ "value": txid })).await;
        if info.get("blockNumber").is_some() {
            assert_ne!(info["receipt"]["result"].as_str(), Some("REVERT"), "{info}");
            return info;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("transaction {txid} is not in a block");
}

async fn tron_deploy_token(funder: &str) -> String {
    use alloy::sol_types::SolConstructor;
    let owner = turmite::payouts::tron::address_from_secret(&hex::decode(funder).unwrap()).unwrap();
    let args = MockErc20::constructorCall { name: "Mock USDT".into(), symbol: "USDT".into(), decimals: 6 }.abi_encode();
    let tx = tron_post(
        "/wallet/deploycontract",
        json!({
            "owner_address": owner, "abi": "[]", "bytecode": MOCK_ERC20_CODE.trim_start_matches("0x"),
            "parameter": hex::encode(args), "fee_limit": 1_000_000_000u64, "call_value": 0,
            "consume_user_resource_percent": 100, "origin_energy_limit": 10_000_000u64, "name": "MockUSDT", "visible": true,
        }),
    )
    .await;
    let info = tron_sign_broadcast(funder, tx).await;
    tron_b58(&hex::decode(info["contract_address"].as_str().unwrap()).unwrap())
}

async fn tron_call(funder: &str, contract: &str, selector: &str, parameter: &str) {
    let owner = turmite::payouts::tron::address_from_secret(&hex::decode(funder).unwrap()).unwrap();
    let r = tron_post(
        "/wallet/triggersmartcontract",
        json!({ "owner_address": owner, "contract_address": contract, "function_selector": selector, "parameter": parameter,
                "fee_limit": 1_000_000_000u64, "call_value": 0, "visible": true }),
    )
    .await;
    tron_sign_broadcast(funder, r["transaction"].clone()).await;
}

async fn tron_token_balance(contract: &str, owner: &str) -> u128 {
    let r = tron_post(
        "/wallet/triggerconstantcontract",
        json!({ "owner_address": owner, "contract_address": contract, "function_selector": "balanceOf(address)",
                "parameter": tron_word_address(owner), "visible": true }),
    )
    .await;
    u128::from_str_radix(&r["constant_result"][0].as_str().unwrap()[32..], 16).unwrap()
}

async fn tron_post_ok() -> Option<()> {
    reqwest::Client::new().post(format!("{TRE}/wallet/getnowblock")).timeout(Duration::from_secs(3)).send().await.ok().map(|_| ())
}

// ---- TON -------------------------------------------------------------------------------------

const TON_API: &str = "http://127.0.0.1:8082";
/// MyLocalTon faucet: wallet v3r2, id 42, masterchain (public test key, see deploy/testnets/README.md).
const TON_FAUCET_SECRET: &str = "a51e8fb6f0fae3834bf430f5012589d319e7b3b3303ceb82c816b762fccf2d05";

async fn ton_balance(address: &str) -> u64 {
    let v: Value = reqwest::Client::new()
        .get(format!("{TON_API}/getAddressBalance"))
        .query(&[("address", address)])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    v["result"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0)
}

#[tokio::test]
async fn ton_payout_on_mylocalton() {
    if !common::chain_enabled("ton") {
        return;
    }
    let alive = reqwest::Client::new().get(format!("{TON_API}/getMasterchainInfo")).timeout(Duration::from_secs(3)).send().await;
    if alive.is_err() {
        eprintln!("SKIP: the local TON API is not running on {TON_API}");
        return;
    }
    let t = common::start(Default::default()).await;
    let (pid, uid, hot_address) = setup(&t, "ton", "ton", TON_API).await;
    let funder = turmite::chains::ton::TonDriver::connect(TON_API, None, true, 1).await.unwrap();
    // The faucet is shared: retry when another test used its seqno at the same time.
    let mut funded = false;
    for _ in 0..5 {
        if funder
            .send_from_secret(
                TON_FAUCET_SECRET,
                tonlib_core::wallet::wallet_version::WalletVersion::V3R2,
                42,
                -1,
                &hot_address,
                5_000_000_000,
            )
            .await
            .is_ok()
        {
            for _ in 0..60 {
                if ton_balance(&hot_address).await >= 5_000_000_000 {
                    funded = true;
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
        if funded {
            break;
        }
    }
    assert!(funded, "the faucet did not fund {hot_address}");

    // The first payout also deploys the hot wallet.
    let to = hot::derive_address("ton", &hot::generate_secret("ton").unwrap(), true).unwrap();
    let id = pay(&t, pid, uid, "ton", &to, "1.5").await.unwrap();
    let p = settle(&t, id).await;
    assert_eq!(p.status, PayoutStatus::Sent, "{:?}", p.error);
    assert!((1_499_990_000..=1_500_000_000).contains(&ton_balance(&to).await), "storage fees aside");
    assert!(p.fee.unwrap() > Decimal::ZERO);
    // The next one uses the next seqno, with a memo.
    let w = NewWithdrawal {
        address: to.clone(),
        currency: "ton".into(),
        amount: "0.25".parse().unwrap(),
        extra_id: Some("order 42".into()),
        ..Default::default()
    };
    let id = service::create_batch(&t.app, pid, Source::Dashboard { user_id: uid }, vec![w], None).await.unwrap().payouts[0].payout_id;
    let p = settle(&t, id).await;
    assert_eq!(p.status, PayoutStatus::Sent, "{:?}", p.error);
    assert!((1_749_990_000..=1_750_000_000).contains(&ton_balance(&to).await), "storage fees aside");
}
