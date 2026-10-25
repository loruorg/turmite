---
title: Tickers and networks
description: Supported networks, tickers, token contracts, chain IDs and default confirmations.
sidebarTitle: Tickers and networks
---

Turmite supports nine networks. Each install runs in one mode, set with `TURMITE_MODE`: `mainnet` or `testnet`. The mode decides which networks Turmite creates in the database. A database keeps its mode: Turmite does not start on it with the other mode.

## Networks

| Code | Network | Testnet | Family | Assets |
|---|---|---|---|---|
| `BTC` | Bitcoin | Signet (regtest for local tests) | bitcoin | BTC |
| `XMR` | Monero | Stagenet | monero | XMR |
| `ETH` | Ethereum | Sepolia | evm | ETH, USDC, USDT |
| `BASE` | Base | Base Sepolia | evm | ETH, USDC |
| `POLYGON` | Polygon | Amoy | evm | POL, USDC, USDT |
| `BSC` | BNB Smart Chain | BSC testnet | evm | BNB, USDC, USDT |
| `TRON` | Tron | Nile | tron | TRX, USDT |
| `SOL` | Solana | Devnet | solana | SOL, USDC, USDT |
| `TON` | TON | Testnet | ton | TON |

Bitcoin and Monero payments go straight to your own wallet. On the other networks, Turmite sweeps each deposit to your address. See [Wallets per chain](/docs/wallets/#custody-per-chain).

## Tickers

Use these codes in `pay_currency` and in payout `currency`. They are the codes of the common hosted-gateway API.

| Network | Ticker | Asset | Decimals |
|---|---|---|---|
| `BTC` | `btc` | BTC | 8 |
| `XMR` | `xmr` | XMR | 12 |
| `ETH` | `eth` | ETH | 18 |
| `ETH` | `usdc` | USDC | 6 |
| `ETH` | `usdterc20` | USDT | 6 |
| `BASE` | `ethbase` | ETH | 18 |
| `BASE` | `usdcbase` | USDC | 6 |
| `POLYGON` | `pol` | POL | 18 |
| `POLYGON` | `usdcmatic` | USDC | 6 |
| `POLYGON` | `usdtmatic` | USDT | 6 |
| `BSC` | `bnbbsc` | BNB | 18 |
| `BSC` | `usdcbsc` | USDC | 18 |
| `BSC` | `usdtbsc` | USDT | 18 |
| `TRON` | `trx` | TRX | 6 |
| `TRON` | `usdttrc20` | USDT | 6 |
| `SOL` | `sol` | SOL | 9 |
| `SOL` | `usdcsol` | USDC | 6 |
| `SOL` | `usdtsol` | USDT | 6 |
| `TON` | `ton` | TON | 9 |

`GET /v1/currencies` returns the tickers that your project accepts. See [Merchant API](/docs/merchant-api/#status-and-currencies).

A global admin can turn each ticker on or off, and add a token contract, on the **Networks** page. See [Networks and nodes](/docs/nodes/#tokens).

## EVM chain IDs

| Network | Mainnet chain ID | Testnet | Testnet chain ID |
|---|---|---|---|
| Ethereum | 1 | Sepolia | 11155111 |
| Base | 8453 | Base Sepolia | 84532 |
| Polygon | 137 | Amoy | 80002 |
| BNB Smart Chain | 56 | BSC testnet | 97 |

Turmite refuses an EVM node whose chain ID is different.

## Token contracts on mainnet

| Ticker | Contract |
|---|---|
| `usdc` | `0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48` |
| `usdterc20` | `0xdAC17F958D2ee523a2206206994597C13D831ec7` |
| `usdcbase` | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` |
| `usdcmatic` | `0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359` |
| `usdtmatic` | `0xc2132D05D31c914a87C6611C10748AEb04B58e8F` |
| `usdcbsc` | `0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d` |
| `usdtbsc` | `0x55d398326f99059fF775485246999027B3197955` |
| `usdttrc20` | `TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t` |
| `usdcsol` | `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` |
| `usdtsol` | `Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB` |

## Tokens on testnet

A testnet install has fewer tokens:

| Network | Testnet tokens |
|---|---|
| Ethereum Sepolia | `usdc` (`0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238`) |
| Base Sepolia | `usdcbase` (`0x036CbD53842c5426634e7929541eC2318f3dCF7e`) |
| Polygon Amoy | `usdcmatic` (`0x41E94Eb019C0762f9Bfcf9Fb1E58725BfB0e7582`) |
| BNB Smart Chain testnet | None |
| Tron Nile | `usdttrc20` (`TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf`) |
| Solana Devnet | `usdcsol` (`4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU`) |

The catalog in `crates/turmite/src/chains/catalog.rs` holds the networks, chain IDs and token contracts for each mode. Turmite writes it to the database on the first start. Later starts keep the changes that admins made.

## Confirmations

A deposit counts when it has the confirmation count of its network. A global admin changes the count on the **Networks** page. See [Networks and nodes](/docs/nodes/#confirmations).

| Network | Mainnet default | Testnet default |
|---|---|---|
| Bitcoin | 2 | 1 |
| Monero | 10 | 10 |
| Ethereum | 12 | 3 |
| Base | 10 | 3 |
| Polygon | 64 | 3 |
| BNB Smart Chain | 15 | 3 |
| Tron | 19 | 1 |
| Solana | 32 | 1 |
| TON | 1 | 1 |

## Network notes

- Bitcoin: deposit addresses come from a BIP84 account key (`xpub` or `zpub` on mainnet, `tpub` or `vpub` on the test networks). Check the first addresses against your wallet. See [Wallets per chain](/docs/wallets/#bitcoin).
- Monero: each payment gets a subaddress of your wallet. Outputs unlock after 10 blocks.
- EVM: the deposit contract does not exist on the chain until the first sweep deploys it. The address can receive funds before that.
- Tron: a deposit counts only when its block is solidified.
- Solana: a `finalized` transaction counts as 32 confirmations. A `confirmed` transaction counts as 1 to 31.
- TON: a block is final when the masterchain refers to it, and the node lists only such transactions. Thus a deposit counts as confirmed when the node lists it, and the default count is 1. Deposit addresses are wallets that are not yet deployed. Turmite shows them in the non-bounceable form (`UQ…` on mainnet, `0Q…` on testnet). The first sweep deploys the wallet.
