#![cfg(feature = "tron")]
//! Tron on the local TRE node (127.0.0.1:9090), in split roles. A customer pays TRX and
//! a TRC-20 token (a test token in place of USDT); the worker confirms, finishes and
//! sweeps. The TRC-20 sweep takes its energy and bandwidth TRX from the hot wallet.

mod common;
mod roles;

use std::time::Duration;

use roles::{Split, random_secret};
use rust_decimal::Decimal;
use serde_json::json;
use turmite::chains::{
    AssetContext, PaymentDriver,
    driver::WalletContext,
    tron::{TronDriver, address_from_secret},
};

const TRE: &str = "http://127.0.0.1:9090";

/// A TRC-20 token with 6 decimals that mints 10^15 units to its deployer (Solidity
/// 0.8.30). The node's history API lists only contracts with the full TRC-20 ABI.
const TOKEN_CODE: &str = "60c0604052600b60809081526a2a32b9ba102a32ba3432b960a91b60a05260009061002a908261015b565b50604080518082019091526005815264151554d11560da1b6020820152600190610054908261015b565b5034801561006157600080fd5b5066038d7ea4c68000600281905533600081815260036020908152604080832085905551938452919290917fddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef910160405180910390a3610219565b634e487b7160e01b600052604160045260246000fd5b600181811c908216806100e657607f821691505b60208210810361010657634e487b7160e01b600052602260045260246000fd5b50919050565b601f82111561015657806000526020600020601f840160051c810160208510156101335750805b601f840160051c820191505b81811015610153576000815560010161013f565b50505b505050565b81516001600160401b03811115610174576101746100bc565b6101888161018284546100d2565b8461010c565b6020601f8211600181146101bc57600083156101a45750848201515b600019600385901b1c1916600184901b178455610153565b600084815260208120601f198516915b828110156101ec57878501518255602094850194600190920191016101cc565b508482101561020a5786840151600019600387901b60f8161c191681555b50505050600190811b01905550565b6105ef806102286000396000f3fe608060405234801561001057600080fd5b50600436106100935760003560e01c8063313ce56711610066578063313ce5671461010357806370a082311461011d57806395d89b411461013d578063a9059cbb14610145578063dd62ed3e1461015857600080fd5b806306fdde0314610098578063095ea7b3146100b657806318160ddd146100d957806323b872dd146100f0575b600080fd5b6100a0610183565b6040516100ad9190610453565b60405180910390f35b6100c96100c43660046104bd565b610211565b60405190151581526020016100ad565b6100e260025481565b6040519081526020016100ad565b6100c96100fe3660046104e7565b61027e565b61010b600681565b60405160ff90911681526020016100ad565b6100e261012b366004610524565b60036020526000908152604090205481565b6100a0610330565b6100c96101533660046104bd565b61033d565b6100e2610166366004610546565b600460209081526000928352604080842090915290825290205481565b6000805461019090610579565b80601f01602080910402602001604051908101604052809291908181526020018280546101bc90610579565b80156102095780601f106101de57610100808354040283529160200191610209565b820191906000526020600020905b8154815290600101906020018083116101ec57829003601f168201915b505050505081565b3360008181526004602090815260408083206001600160a01b038716808552925280832085905551919290917f8c5be1e5ebec7d5bd14f71427d1e84f3dd0314c0f7b2291e5b200ac8c7c3b9259061026c9086815260200190565b60405180910390a35060015b92915050565b6001600160a01b03831660009081526004602090815260408083203384529091528120548211156102e25760405162461bcd60e51b8152602060048201526009602482015268616c6c6f77616e636560b81b60448201526064015b60405180910390fd5b6001600160a01b0384166000908152600460209081526040808320338452909152812080548492906103159084906105c9565b909155506103269050848484610353565b5060019392505050565b6001805461019090610579565b600061034a338484610353565b50600192915050565b6001600160a01b0383166000908152600360205260409020548111156103a55760405162461bcd60e51b815260206004820152600760248201526662616c616e636560c81b60448201526064016102d9565b6001600160a01b038316600090815260036020526040812080548392906103cd9084906105c9565b90915550506001600160a01b038216600090815260036020526040812080548392906103fa9084906105dc565b92505081905550816001600160a01b0316836001600160a01b03167fddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef8360405161044691815260200190565b60405180910390a3505050565b602081526000825180602084015260005b818110156104815760208186018101516040868401015201610464565b506000604082850101526040601f19601f83011684010191505092915050565b80356001600160a01b03811681146104b857600080fd5b919050565b600080604083850312156104d057600080fd5b6104d9836104a1565b946020939093013593505050565b6000806000606084860312156104fc57600080fd5b610505846104a1565b9250610513602085016104a1565b929592945050506040919091013590565b60006020828403121561053657600080fd5b61053f826104a1565b9392505050565b6000806040838503121561055957600080fd5b610562836104a1565b9150610570602084016104a1565b90509250929050565b600181811c9082168061058d57607f821691505b6020821081036105ad57634e487b7160e01b600052602260045260246000fd5b50919050565b634e487b7160e01b600052601160045260246000fd5b81810381811115610278576102786105b3565b80820180821115610278576102786105b356";

const TOKEN_ABI: &str = r#"[{"type":"constructor","inputs":[],"stateMutability":"nonpayable"},{"type":"function","name":"allowance","inputs":[{"name":"","type":"address"},{"name":"","type":"address"}],"outputs":[{"name":"","type":"uint256"}],"stateMutability":"view"},{"type":"function","name":"approve","inputs":[{"name":"spender","type":"address"},{"name":"amount","type":"uint256"}],"outputs":[{"name":"","type":"bool"}],"stateMutability":"nonpayable"},{"type":"function","name":"balanceOf","inputs":[{"name":"","type":"address"}],"outputs":[{"name":"","type":"uint256"}],"stateMutability":"view"},{"type":"function","name":"decimals","inputs":[],"outputs":[{"name":"","type":"uint8"}],"stateMutability":"view"},{"type":"function","name":"name","inputs":[],"outputs":[{"name":"","type":"string"}],"stateMutability":"view"},{"type":"function","name":"symbol","inputs":[],"outputs":[{"name":"","type":"string"}],"stateMutability":"view"},{"type":"function","name":"totalSupply","inputs":[],"outputs":[{"name":"","type":"uint256"}],"stateMutability":"view"},{"type":"function","name":"transfer","inputs":[{"name":"to","type":"address"},{"name":"amount","type":"uint256"}],"outputs":[{"name":"","type":"bool"}],"stateMutability":"nonpayable"},{"type":"function","name":"transferFrom","inputs":[{"name":"from","type":"address"},{"name":"to","type":"address"},{"name":"amount","type":"uint256"}],"outputs":[{"name":"","type":"bool"}],"stateMutability":"nonpayable"},{"type":"event","name":"Approval","inputs":[{"name":"owner","type":"address","indexed":true},{"name":"spender","type":"address","indexed":true},{"name":"value","type":"uint256","indexed":false}],"anonymous":false},{"type":"event","name":"Transfer","inputs":[{"name":"from","type":"address","indexed":true},{"name":"to","type":"address","indexed":true},{"name":"value","type":"uint256","indexed":false}],"anonymous":false}]"#;

/// Private keys of the prefunded accounts (`/admin/accounts`).
async fn prefunded_keys() -> Vec<String> {
    let page = match reqwest::get(format!("{TRE}/admin/accounts")).await {
        Ok(r) => r.text().await.unwrap_or_default(),
        Err(_) => return vec![],
    };
    page.lines()
        .filter_map(|l| l.trim().strip_prefix('(').and_then(|r| r.split_once(") ")).map(|(_, r)| r.trim().to_string()))
        .filter(|t| t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit()))
        .collect()
}

async fn wait_trx(chain: &TronDriver, address: &str, at_least: u64) -> u64 {
    let mut b = 0;
    for _ in 0..40 {
        b = chain.balance_sun(address).await.unwrap_or(0);
        if b >= at_least {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
    b
}

#[tokio::test]
async fn tron_split_roles_trx_and_trc20_sweeps() {
    let keys = prefunded_keys().await;
    if keys.is_empty() {
        eprintln!("SKIP: TRE not reachable at {TRE}");
        return;
    }
    let funder = hex::decode(&keys[0]).unwrap();
    let chain = TronDriver::connect(TRE, None, true, 1).await.unwrap();
    assert!(chain.is_indexed(), "TRE serves the TronGrid-style history API");
    let err = TronDriver::connect(TRE, None, false, 1).await.err().expect("a local chain is refused in mainnet mode");
    assert!(err.to_string().contains("not on mainnet"), "{err}");

    // Settlement wallet (activated) and hot wallet.
    let settle = address_from_secret(&random_secret()).unwrap();
    let hot_secret = random_secret();
    let hot = address_from_secret(&hot_secret).unwrap();
    chain.send_trx_from_key(&funder, &settle, 1_000_000).await.unwrap();
    chain.send_trx_from_key(&funder, &hot, 200_000_000).await.unwrap();
    assert!(wait_trx(&chain, &hot, 200_000_000).await >= 200_000_000, "hot wallet funded");
    assert!(wait_trx(&chain, &settle, 1_000_000).await >= 1_000_000, "settlement activated");

    let s = Split::start("TRON", TRE).await;
    let (_pid, key) = s.project("TRON", &settle, None, Some(("tron", &hot, &hot_secret))).await;
    turmite::worker::pool::refill(&s.worker).await;

    // TRX: pay 3 TRX, confirm, finish, sweep.
    let (status, p, _) = s.post("/v1/payment", &key, &json!({ "price_amount": 3, "price_currency": "usd", "pay_currency": "trx" })).await;
    assert_eq!(status, 201, "{p}");
    let id = common::pid(&p);
    let deposit = p["pay_address"].as_str().unwrap().to_string();
    let worker_driver = s.worker.drivers().get("tron").unwrap();
    assert_eq!(worker_driver.derive_deposit(&WalletContext::default(), s.deposit_index(id).await).await.unwrap().address, deposit);
    assert_eq!(p["pay_amount"].as_f64().unwrap(), 3.0);
    chain.send_trx_from_key(&funder, &deposit, 3_000_000).await.unwrap();
    let (p, sweeps) = s.settle(&key, id, 120).await;
    assert_eq!(p["payment_status"], "finished", "{p}");
    assert_eq!(sweeps.len(), 1);
    assert_eq!(p["outcome_amount"].as_f64().unwrap(), 3.0);
    let settle_trx = wait_trx(&chain, &settle, 4_000_000).await;
    assert_eq!(settle_trx, 4_000_000, "settlement received 3 TRX");
    assert_eq!(chain.balance_sun(&deposit).await.unwrap(), 0, "deposit emptied");
    println!("TRX sweep {}: fee {} TRX", sweeps[0].txid.as_deref().unwrap_or(""), sweeps[0].network_fee_amount);
    assert_eq!(s.sweeps_final(id, 120).await[0].status, "confirmed", "the sweep is in a solid block");

    // TRC-20: a test token in place of USDT.
    let token = chain.deploy_contract(&funder, "TestToken", &hex::decode(TOKEN_CODE).unwrap(), TOKEN_ABI).await.unwrap();
    s.set_token_contract("usdttrc20", &token).await;
    let (status, p, _) =
        s.post("/v1/payment", &key, &json!({ "price_amount": 5, "price_currency": "usd", "pay_currency": "usdttrc20" })).await;
    assert_eq!(status, 201, "{p}");
    let id = common::pid(&p);
    let deposit = p["pay_address"].as_str().unwrap().to_string();
    chain.send_token_from_key(&funder, &token, &deposit, 5_000_000).await.unwrap();
    let hot_before = chain.balance_sun(&hot).await.unwrap();
    let (p, sweeps) = s.settle(&key, id, 180).await;
    assert_eq!(p["payment_status"], "finished", "{p}");
    assert_eq!(p["outcome_amount"].as_f64().unwrap(), 5.0);
    assert_eq!(sweeps.len(), 1);
    assert_eq!(chain.token_balance(&token, &settle).await.unwrap(), 5_000_000, "settlement received the tokens");
    assert_eq!(chain.token_balance(&token, &deposit).await.unwrap(), 0, "deposit emptied");
    assert_eq!(s.sweeps_final(id, 120).await[0].status, "confirmed");
    let hot_after = chain.balance_sun(&hot).await.unwrap();
    println!(
        "TRC-20 sweep {}: hot wallet paid {} TRX; {} TRX left on the deposit",
        sweeps[0].txid.as_deref().unwrap_or(""),
        Decimal::from(hot_before - hot_after) / Decimal::from(1_000_000),
        Decimal::from(chain.balance_sun(&deposit).await.unwrap()) / Decimal::from(1_000_000)
    );

    // A node without the history API: the driver reads balances instead.
    let mut plain = TronDriver::connect(TRE, None, true, 1).await.unwrap();
    plain.set_indexed(false);
    let (_, p, _) = s.post("/v1/payment", &key, &json!({ "price_amount": 5, "price_currency": "usd", "pay_currency": "usdttrc20" })).await;
    let deposit = p["pay_address"].as_str().unwrap().to_string();
    chain.send_token_from_key(&funder, &token, &deposit, 2_500_000).await.unwrap();
    let asset = AssetContext { decimals: 6, contract_address: Some(token.clone()) };
    let mut seen = vec![];
    for _ in 0..20 {
        seen = plain.observe(&WalletContext::default(), &deposit, None, &asset).await.unwrap();
        if seen.first().is_some_and(|o| o.confirmations > 0) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].amount, Decimal::new(25, 1));
    assert_eq!(seen[0].confirmations, 1);
    let indexed = chain.observe(&WalletContext::default(), &deposit, None, &asset).await.unwrap();
    assert_eq!(indexed.len(), 1);
    assert_eq!(indexed[0].amount, Decimal::new(25, 1));
    assert!(chain.tip_height().await.unwrap() > 1);
}
