//! EVM deposit contracts (`contracts/`): one deposit address per payment. The funds at a deposit address
//! can only go to the cold wallet that is fixed in the project's `DepositFactory`. The hot key only pays gas.
//!
//! Addresses, all computed off-chain:
//! - factory: CREATE2 by the deployer at `0x4e59…956c` (present on every supported chain and on anvil),
//!   salt zero, init code = creation code ++ `abi.encode(cold_wallet)`. The same cold wallet gives the
//!   same factory address on every EVM chain.
//! - implementation: CREATE by the factory at nonce 1.
//! - deposit address: CREATE2 by the factory with `salt_for_index(payment_id)` and the EIP-1167 clone init
//!   code. It can receive funds before the clone exists. `flush` deploys the clone when necessary.
//!
//! A deposit address does not depend on the chain. Use one index space for all payments that share a cold
//! wallet (the payment id), so that two payments never get the same address on any chain.

use std::sync::LazyLock;

use alloy::primitives::{Address, B256, Bytes, address, hex, keccak256};
use alloy::rpc::types::{TransactionInput, TransactionRequest};
use alloy::sol_types::{SolCall, SolConstructor};

const ARTIFACT: &str = include_str!("../../../../contracts/out-min/DepositFactory.json");

/// Deterministic CREATE2 deployer (`salt ++ init code` in, address out).
pub const CREATE2_DEPLOYER: Address = address!("4e59b44847b379578588920ca78fbf26c0b4956c");
/// Salt of the factory deployment.
pub const FACTORY_SALT: B256 = B256::ZERO;
/// Token address that means the native coin in `flush` and in the events.
pub const NATIVE: Address = Address::ZERO;
/// Salts per flush transaction. One salt with the native coin and two tokens uses about 80k gas, so 150 salts
/// stay below the 2^24 gas cap per transaction on Ethereum, Base and BNB Smart Chain.
pub const MAX_SALTS_PER_FLUSH: usize = 150;

const CLONE_PREFIX: [u8; 20] = hex!("3d602d80600a3d3981f3363d3d373d3d3d363d73");
const CLONE_SUFFIX: [u8; 15] = hex!("5af43d82803e903d91602b57fd5bf3");

alloy::sol! {
    #[sol(rpc)]
    #[allow(missing_docs)]
    contract DepositFactory {
        event Deployed(bytes32 indexed salt, address indexed wallet);
        event Flushed(address indexed wallet, address indexed token, uint256 amount);
        event FlushFailed(address indexed wallet, address indexed token);

        error ZeroColdWallet();
        error TooManyTokens();
        error DeployFailed();

        constructor(address coldWallet_);

        function coldWallet() external view returns (address);
        function implementation() external view returns (address);
        function predict(bytes32 salt) external view returns (address);
        function flush(bytes32[] calldata salts, address[] calldata tokens) external;
    }
}

static CREATION_CODE: LazyLock<Bytes> = LazyLock::new(|| {
    let v: serde_json::Value = serde_json::from_str(ARTIFACT).expect("DepositFactory.json is JSON");
    v["bytecode"].as_str().and_then(|s| s.parse().ok()).expect("DepositFactory.json has hex bytecode")
});

/// Creation code of the factory, without constructor arguments.
pub fn factory_creation_code() -> &'static Bytes {
    &CREATION_CODE
}

/// Creation code with the cold wallet as constructor argument.
pub fn factory_init_code(cold_wallet: Address) -> Bytes {
    let mut code = CREATION_CODE.to_vec();
    code.extend_from_slice(&DepositFactory::constructorCall { coldWallet_: cold_wallet }.abi_encode());
    code.into()
}

/// Address of the factory for a cold wallet. The same on every chain.
pub fn factory_address(cold_wallet: Address) -> Address {
    CREATE2_DEPLOYER.create2(FACTORY_SALT, keccak256(factory_init_code(cold_wallet)))
}

/// Unsigned transaction that deploys the factory through the CREATE2 deployer.
/// Anyone can send it; the result is the same. Check for code at `factory_address` first:
/// a second deployment reverts.
pub fn deploy_factory_tx(cold_wallet: Address) -> TransactionRequest {
    let mut input = FACTORY_SALT.to_vec();
    input.extend_from_slice(&factory_init_code(cold_wallet));
    TransactionRequest { to: Some(CREATE2_DEPLOYER.into()), input: TransactionInput::both(input.into()), ..Default::default() }
}

/// The DepositWallet implementation that the factory deploys in its constructor.
pub fn implementation_address(factory: Address) -> Address {
    factory.create(1)
}

/// EIP-1167 init code of a clone of `implementation`.
pub fn clone_init_code(implementation: Address) -> [u8; 55] {
    let mut code = [0u8; 55];
    code[..20].copy_from_slice(&CLONE_PREFIX);
    code[20..40].copy_from_slice(implementation.as_slice());
    code[40..].copy_from_slice(&CLONE_SUFFIX);
    code
}

/// Deposit address for a salt. Same result as `DepositFactory.predict(salt)`.
pub fn predict_deposit_address(factory: Address, implementation: Address, salt: B256) -> Address {
    factory.create2(salt, keccak256(clone_init_code(implementation)))
}

/// Salt for a payment: `bytes32(uint256(index))`, big-endian.
pub fn salt_for_index(index: u64) -> B256 {
    B256::left_padding_from(&index.to_be_bytes())
}

/// Calldata of `flush(salts, tokens)`. Put `NATIVE` in `tokens` to flush the native coin.
/// At most `MAX_SALTS_PER_FLUSH` salts and 256 tokens per call.
pub fn flush_call(salts: &[B256], tokens: &[Address]) -> Bytes {
    DepositFactory::flushCall { salts: salts.to_vec(), tokens: tokens.to_vec() }.abi_encode().into()
}

#[cfg(test)]
mod tests {
    use alloy::json_abi::JsonAbi;
    use alloy::primitives::{TxKind, b256};
    use alloy::sol_types::{SolError, SolEvent};

    use super::*;

    // Vectors from contracts/test/unit/Vectors.t.sol.
    const COLD: Address = address!("00000000000000000000000000000000c01d0001");
    const FACTORY: Address = address!("3AB39560dF2B6336624F50a135Ad4D3Ad1161892");
    const IMPLEMENTATION: Address = address!("b611924EdeD1fc9717921F14B54Ab32E9386dcA3");

    #[test]
    fn factory_and_implementation() {
        assert_eq!(factory_address(COLD), FACTORY);
        assert_eq!(implementation_address(FACTORY), IMPLEMENTATION);
        assert_eq!(keccak256(clone_init_code(IMPLEMENTATION)), b256!("0e9e2845fef024dbbc5c120dffb10a750abf3d4f0449050501156c0e5d1056dd"));
    }

    #[test]
    fn deposit_addresses() {
        let cases = [
            (0, address!("170b0A3c92AFd6Bb16395e431CA179f43e9001d7")),
            (1, address!("B74e7c90F860d2a9ccD374D4E6f6fAf0061615f4")),
            (42, address!("1f50D60B339b180d66eAd7212052579790a2D450")),
            (u64::MAX, address!("AeE9Cd4e6AbAb719E6Cc0Fd428c0e29f9Fac01a3")),
        ];
        for (index, want) in cases {
            assert_eq!(predict_deposit_address(FACTORY, IMPLEMENTATION, salt_for_index(index)), want, "{index}");
        }
        let full = keccak256("turmite");
        assert_eq!(predict_deposit_address(FACTORY, IMPLEMENTATION, full), address!("1083Aa379CCF763B7d46fA50abdaEE5dD64f9F47"));
        assert_eq!(
            predict_deposit_address(
                address!("5FbDB2315678afecb367f032d93F642f64180aa3"),
                address!("e7f1725E7734CE288F8367e1Bb143E90bb3F0512"),
                salt_for_index(1),
            ),
            address!("82f890a4981b94E87d37Cb55CFb07a8E16c3937f")
        );
    }

    #[test]
    fn salt_is_big_endian_index() {
        assert_eq!(salt_for_index(0), B256::ZERO);
        assert_eq!(salt_for_index(258), b256!("0000000000000000000000000000000000000000000000000000000000000102"));
    }

    #[test]
    fn deploy_tx() {
        let tx = deploy_factory_tx(COLD);
        assert_eq!(tx.to, Some(TxKind::Call(CREATE2_DEPLOYER)));
        let input = tx.input.input().expect("input");
        assert_eq!(&input[..32], FACTORY_SALT.as_slice());
        assert_eq!(input[32..], factory_init_code(COLD)[..]);
        assert_eq!(input.len(), 32 + factory_creation_code().len() + 32);
        assert_eq!(&input[input.len() - 20..], COLD.as_slice());
    }

    #[test]
    fn flush_calldata() {
        let salts = [salt_for_index(1), salt_for_index(2)];
        let tokens = [NATIVE, address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48")];
        let data = flush_call(&salts, &tokens);
        assert_eq!(data[..4], DepositFactory::flushCall::SELECTOR);
        let call = DepositFactory::flushCall::abi_decode(&data).expect("decode");
        assert_eq!(call.salts, salts);
        assert_eq!(call.tokens, tokens);
    }

    /// The `sol!` interface above must match the compiled ABI.
    #[test]
    fn bindings_match_artifact() {
        let v: serde_json::Value = serde_json::from_str(ARTIFACT).unwrap();
        let abi: JsonAbi = serde_json::from_value(v["abi"].clone()).unwrap();
        let sel = |name: &str| abi.function(name).unwrap()[0].selector();
        assert_eq!(sel("flush"), DepositFactory::flushCall::SELECTOR);
        assert_eq!(sel("predict"), DepositFactory::predictCall::SELECTOR);
        assert_eq!(sel("coldWallet"), DepositFactory::coldWalletCall::SELECTOR);
        assert_eq!(sel("implementation"), DepositFactory::implementationCall::SELECTOR);
        let topic = |name: &str| abi.event(name).unwrap()[0].selector();
        assert_eq!(topic("Deployed"), DepositFactory::Deployed::SIGNATURE_HASH);
        assert_eq!(topic("Flushed"), DepositFactory::Flushed::SIGNATURE_HASH);
        assert_eq!(topic("FlushFailed"), DepositFactory::FlushFailed::SIGNATURE_HASH);
        let err = |name: &str| abi.error(name).unwrap()[0].selector();
        assert_eq!(err("ZeroColdWallet"), DepositFactory::ZeroColdWallet::SELECTOR);
        assert_eq!(err("TooManyTokens"), DepositFactory::TooManyTokens::SELECTOR);
        assert_eq!(err("DeployFailed"), DepositFactory::DeployFailed::SELECTOR);
        assert_eq!(abi.functions().count(), 4);
        assert_eq!(abi.events().count(), 3);
        let ctor = abi.constructor().unwrap();
        assert_eq!(ctor.inputs.len(), 1);
        assert_eq!(ctor.inputs[0].ty, "address");
    }
}
