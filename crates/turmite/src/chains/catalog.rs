//! Networks and tokens seeded for each mode. Operators can change
//! confirmations, enable or disable rows and add tokens in the dashboard.

use crate::config::Mode;

pub struct AssetDef {
    pub symbol: &'static str,
    pub name: &'static str,
    pub coingecko_id: &'static str,
    pub is_stable: bool,
}

pub struct TokenDef {
    pub symbol: &'static str,
    pub ticker: &'static str,
    pub decimals: i32,
    pub contract: &'static str,
}

pub struct NetworkDef {
    pub code: &'static str,
    /// Driver key.
    pub chain: &'static str,
    pub family: &'static str,
    pub name: &'static str,
    pub kind: &'static str,
    pub chain_id: Option<i64>,
    pub native_symbol: &'static str,
    pub native_ticker: &'static str,
    pub native_decimals: i32,
    pub confirmations: i32,
    pub explorer_tx: &'static str,
    pub explorer_address: &'static str,
    pub tokens: Vec<TokenDef>,
}

pub const ASSETS: &[AssetDef] = &[
    AssetDef { symbol: "BTC", name: "Bitcoin", coingecko_id: "bitcoin", is_stable: false },
    AssetDef { symbol: "XMR", name: "Monero", coingecko_id: "monero", is_stable: false },
    AssetDef { symbol: "ETH", name: "Ether", coingecko_id: "ethereum", is_stable: false },
    AssetDef { symbol: "POL", name: "POL", coingecko_id: "polygon-ecosystem-token", is_stable: false },
    AssetDef { symbol: "BNB", name: "BNB", coingecko_id: "binancecoin", is_stable: false },
    AssetDef { symbol: "TRX", name: "TRON", coingecko_id: "tron", is_stable: false },
    AssetDef { symbol: "SOL", name: "Solana", coingecko_id: "solana", is_stable: false },
    AssetDef { symbol: "TON", name: "Toncoin", coingecko_id: "the-open-network", is_stable: false },
    AssetDef { symbol: "USDC", name: "USD Coin", coingecko_id: "usd-coin", is_stable: true },
    AssetDef { symbol: "USDT", name: "Tether USD", coingecko_id: "tether", is_stable: true },
];

const fn tok(symbol: &'static str, ticker: &'static str, decimals: i32, contract: &'static str) -> TokenDef {
    TokenDef { symbol, ticker, decimals, contract }
}

pub fn networks(mode: Mode) -> Vec<NetworkDef> {
    match mode {
        Mode::Mainnet => mainnet(),
        Mode::Testnet => testnet(),
    }
}

fn mainnet() -> Vec<NetworkDef> {
    vec![
        NetworkDef {
            code: "BTC",
            chain: "bitcoin",
            family: "bitcoin",
            name: "Bitcoin",
            kind: "utxo",
            chain_id: None,
            native_symbol: "BTC",
            native_ticker: "btc",
            native_decimals: 8,
            confirmations: 2,
            explorer_tx: "https://mempool.space/tx/{txid}",
            explorer_address: "https://mempool.space/address/{address}",
            tokens: vec![],
        },
        NetworkDef {
            code: "XMR",
            chain: "monero",
            family: "monero",
            name: "Monero",
            kind: "utxo",
            chain_id: None,
            native_symbol: "XMR",
            native_ticker: "xmr",
            native_decimals: 12,
            confirmations: 10,
            explorer_tx: "https://xmrchain.net/tx/{txid}",
            explorer_address: "",
            tokens: vec![],
        },
        NetworkDef {
            code: "ETH",
            chain: "ethereum",
            family: "evm",
            name: "Ethereum",
            kind: "evm",
            chain_id: Some(1),
            native_symbol: "ETH",
            native_ticker: "eth",
            native_decimals: 18,
            confirmations: 12,
            explorer_tx: "https://etherscan.io/tx/{txid}",
            explorer_address: "https://etherscan.io/address/{address}",
            tokens: vec![
                tok("USDC", "usdc", 6, "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"),
                tok("USDT", "usdterc20", 6, "0xdAC17F958D2ee523a2206206994597C13D831ec7"),
            ],
        },
        NetworkDef {
            code: "BASE",
            chain: "base",
            family: "evm",
            name: "Base",
            kind: "evm",
            chain_id: Some(8453),
            native_symbol: "ETH",
            native_ticker: "ethbase",
            native_decimals: 18,
            confirmations: 10,
            explorer_tx: "https://basescan.org/tx/{txid}",
            explorer_address: "https://basescan.org/address/{address}",
            tokens: vec![tok("USDC", "usdcbase", 6, "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913")],
        },
        NetworkDef {
            code: "POLYGON",
            chain: "polygon",
            family: "evm",
            name: "Polygon",
            kind: "evm",
            chain_id: Some(137),
            native_symbol: "POL",
            native_ticker: "pol",
            native_decimals: 18,
            confirmations: 64,
            explorer_tx: "https://polygonscan.com/tx/{txid}",
            explorer_address: "https://polygonscan.com/address/{address}",
            tokens: vec![
                tok("USDC", "usdcmatic", 6, "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359"),
                tok("USDT", "usdtmatic", 6, "0xc2132D05D31c914a87C6611C10748AEb04B58e8F"),
            ],
        },
        NetworkDef {
            code: "BSC",
            chain: "bsc",
            family: "evm",
            name: "BNB Smart Chain",
            kind: "evm",
            chain_id: Some(56),
            native_symbol: "BNB",
            native_ticker: "bnbbsc",
            native_decimals: 18,
            confirmations: 15,
            explorer_tx: "https://bscscan.com/tx/{txid}",
            explorer_address: "https://bscscan.com/address/{address}",
            tokens: vec![
                tok("USDC", "usdcbsc", 18, "0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d"),
                tok("USDT", "usdtbsc", 18, "0x55d398326f99059fF775485246999027B3197955"),
            ],
        },
        NetworkDef {
            code: "TRON",
            chain: "tron",
            family: "tron",
            name: "TRON",
            kind: "account",
            chain_id: None,
            native_symbol: "TRX",
            native_ticker: "trx",
            native_decimals: 6,
            confirmations: 19,
            explorer_tx: "https://tronscan.org/#/transaction/{txid}",
            explorer_address: "https://tronscan.org/#/address/{address}",
            tokens: vec![tok("USDT", "usdttrc20", 6, "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t")],
        },
        NetworkDef {
            code: "SOL",
            chain: "solana",
            family: "solana",
            name: "Solana",
            kind: "account",
            chain_id: None,
            native_symbol: "SOL",
            native_ticker: "sol",
            native_decimals: 9,
            confirmations: 32,
            explorer_tx: "https://solscan.io/tx/{txid}",
            explorer_address: "https://solscan.io/account/{address}",
            tokens: vec![
                tok("USDC", "usdcsol", 6, "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"),
                tok("USDT", "usdtsol", 6, "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"),
            ],
        },
        NetworkDef {
            code: "TON",
            chain: "ton",
            family: "ton",
            name: "TON",
            kind: "account",
            chain_id: None,
            native_symbol: "TON",
            native_ticker: "ton",
            native_decimals: 9,
            confirmations: 1,
            explorer_tx: "https://tonviewer.com/transaction/{txid}",
            explorer_address: "https://tonviewer.com/{address}",
            tokens: vec![],
        },
    ]
}

fn testnet() -> Vec<NetworkDef> {
    vec![
        NetworkDef {
            code: "BTC",
            chain: "bitcoin",
            family: "bitcoin",
            name: "Bitcoin Signet",
            kind: "utxo",
            chain_id: None,
            native_symbol: "BTC",
            native_ticker: "btc",
            native_decimals: 8,
            confirmations: 1,
            explorer_tx: "https://mempool.space/signet/tx/{txid}",
            explorer_address: "https://mempool.space/signet/address/{address}",
            tokens: vec![],
        },
        NetworkDef {
            code: "XMR",
            chain: "monero",
            family: "monero",
            name: "Monero Stagenet",
            kind: "utxo",
            chain_id: None,
            native_symbol: "XMR",
            native_ticker: "xmr",
            native_decimals: 12,
            confirmations: 10,
            explorer_tx: "https://stagenet.xmrchain.net/tx/{txid}",
            explorer_address: "",
            tokens: vec![],
        },
        NetworkDef {
            code: "ETH",
            chain: "ethereum",
            family: "evm",
            name: "Ethereum Sepolia",
            kind: "evm",
            chain_id: Some(11155111),
            native_symbol: "ETH",
            native_ticker: "eth",
            native_decimals: 18,
            confirmations: 3,
            explorer_tx: "https://sepolia.etherscan.io/tx/{txid}",
            explorer_address: "https://sepolia.etherscan.io/address/{address}",
            tokens: vec![tok("USDC", "usdc", 6, "0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238")],
        },
        NetworkDef {
            code: "BASE",
            chain: "base",
            family: "evm",
            name: "Base Sepolia",
            kind: "evm",
            chain_id: Some(84532),
            native_symbol: "ETH",
            native_ticker: "ethbase",
            native_decimals: 18,
            confirmations: 3,
            explorer_tx: "https://sepolia.basescan.org/tx/{txid}",
            explorer_address: "https://sepolia.basescan.org/address/{address}",
            tokens: vec![tok("USDC", "usdcbase", 6, "0x036CbD53842c5426634e7929541eC2318f3dCF7e")],
        },
        NetworkDef {
            code: "POLYGON",
            chain: "polygon",
            family: "evm",
            name: "Polygon Amoy",
            kind: "evm",
            chain_id: Some(80002),
            native_symbol: "POL",
            native_ticker: "pol",
            native_decimals: 18,
            confirmations: 3,
            explorer_tx: "https://amoy.polygonscan.com/tx/{txid}",
            explorer_address: "https://amoy.polygonscan.com/address/{address}",
            tokens: vec![tok("USDC", "usdcmatic", 6, "0x41E94Eb019C0762f9Bfcf9Fb1E58725BfB0e7582")],
        },
        NetworkDef {
            code: "BSC",
            chain: "bsc",
            family: "evm",
            name: "BNB Smart Chain Testnet",
            kind: "evm",
            chain_id: Some(97),
            native_symbol: "BNB",
            native_ticker: "bnbbsc",
            native_decimals: 18,
            confirmations: 3,
            explorer_tx: "https://testnet.bscscan.com/tx/{txid}",
            explorer_address: "https://testnet.bscscan.com/address/{address}",
            tokens: vec![],
        },
        NetworkDef {
            code: "TRON",
            chain: "tron",
            family: "tron",
            name: "TRON Nile",
            kind: "account",
            chain_id: None,
            native_symbol: "TRX",
            native_ticker: "trx",
            native_decimals: 6,
            confirmations: 1,
            explorer_tx: "https://nile.tronscan.org/#/transaction/{txid}",
            explorer_address: "https://nile.tronscan.org/#/address/{address}",
            tokens: vec![tok("USDT", "usdttrc20", 6, "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf")],
        },
        NetworkDef {
            code: "SOL",
            chain: "solana",
            family: "solana",
            name: "Solana Devnet",
            kind: "account",
            chain_id: None,
            native_symbol: "SOL",
            native_ticker: "sol",
            native_decimals: 9,
            confirmations: 1,
            explorer_tx: "https://solscan.io/tx/{txid}?cluster=devnet",
            explorer_address: "https://solscan.io/account/{address}?cluster=devnet",
            tokens: vec![tok("USDC", "usdcsol", 6, "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU")],
        },
        NetworkDef {
            code: "TON",
            chain: "ton",
            family: "ton",
            name: "TON Testnet",
            kind: "account",
            chain_id: None,
            native_symbol: "TON",
            native_ticker: "ton",
            native_decimals: 9,
            confirmations: 1,
            explorer_tx: "https://testnet.tonviewer.com/transaction/{txid}",
            explorer_address: "https://testnet.tonviewer.com/{address}",
            tokens: vec![],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tickers_are_unique_per_mode() {
        for mode in [Mode::Mainnet, Mode::Testnet] {
            let mut seen = std::collections::HashSet::new();
            for n in networks(mode) {
                assert!(seen.insert(n.native_ticker), "{}", n.native_ticker);
                for t in &n.tokens {
                    assert!(seen.insert(t.ticker), "{}", t.ticker);
                    assert!(ASSETS.iter().any(|a| a.symbol == t.symbol));
                }
                assert!(ASSETS.iter().any(|a| a.symbol == n.native_symbol));
            }
        }
    }
}
