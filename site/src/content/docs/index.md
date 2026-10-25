---
title: Overview
description: Turmite is a free, self-hosted crypto payment gateway that you run on your own server.
---

Turmite is a crypto payment gateway that you run on your own server. It is free software under the MIT license. Its community maintains it.

There is no hosted service. There are no fees and no accounts on turmite.com. One install belongs to one operator and can serve any number of projects (shops).

## How a payment works

1. Your shop sends `POST /v1/invoice` or `POST /v1/payment` to your Turmite server.
2. Turmite reserves a deposit address for the payment.
3. The customer opens the checkout page. The page shows the amount, the address and a QR code.
4. The customer pays from a wallet or an exchange.
5. The worker sees the transaction. The status changes to `confirming`.
6. After the required confirmations, the status changes to `confirmed`, and then to `finished` in the same pass. Your shop ships the order.
7. On chains with a sweep, the worker then moves the funds to your cold wallet. The sweep does not change the payment status.

Turmite sends a signed webhook to your shop at each status change. See [Webhooks](/docs/webhooks/) and [Payment statuses](/docs/statuses/).

## Custody per chain

Turmite does not hold your funds where the chain allows it. On Bitcoin, Monero and the EVM networks, the destination of the funds is fixed before the customer pays.

| Network | Deposit address | Where the funds go | Spending key on the server |
|---|---|---|---|
| Bitcoin | Derived from your BIP84 account key, watch-only | Directly into your wallet. No sweep | None |
| Monero | A subaddress of your wallet, from your address and private view key (view-only) | Directly into your wallet. No sweep | None. The view key can see payments but cannot spend |
| Ethereum, Base, Polygon, BNB Smart Chain | A CREATE2 deposit contract per payment | The contract can only flush to the cold wallet fixed in your project's factory | None for deposits. The hot wallet pays gas |
| Tron, Solana, TON | A key per payment, derived from the install's deposit seed | Swept to your cold wallet | The deposit seed, encrypted. Only the worker can decrypt it |

On Tron, Solana and TON, funds wait at the deposit address until the sweep. A full compromise of the server during that time can reach them. See [Security model](/docs/security-model/).

Hot wallets pay network fees and send payouts within the limits that you set. Keep their balance small. See [Payouts](/docs/payouts/).

## Supported networks

| Network | Tickers |
|---|---|
| Bitcoin | `btc` |
| Monero | `xmr` |
| Ethereum | `eth`, `usdc`, `usdterc20` |
| Base | `ethbase`, `usdcbase` |
| Polygon | `pol`, `usdcmatic`, `usdtmatic` |
| BNB Smart Chain | `bnbbsc`, `usdcbsc`, `usdtbsc` |
| Tron | `trx`, `usdttrc20` |
| Solana | `sol`, `usdcsol`, `usdtsol` |
| TON | `ton` |

Each install runs in one mode: `mainnet` or `testnet`. Testnet mode uses Signet, Stagenet, Sepolia, Base Sepolia, Amoy, BSC testnet, Nile, Devnet and the TON testnet. See [Tickers and networks](/docs/networks/).

## Features

- A merchant API with the same paths, fields and IPN signature as the common hosted-gateway API. An existing integration changes only the base URL and the keys. See [Migrate from a hosted gateway](/docs/migrate/).
- Invoices and payments with a hosted checkout. The checkout works with JavaScript off.
- Two kinds of webhooks: a per-payment IPN callback and project endpoints with a timestamped signature.
- Payouts from hot wallets on the EVM networks, Solana, Tron and TON, with an address book, limits in USD, and approval with a two-factor code and an emailed code.
- Optional payment with any coin through [Trocador](https://trocador.app): the customer pays with a coin that Trocador lists, and an exchange delivers a coin that the project accepts. See [Pay with any coin](/docs/exchange/).
- Projects, each with its own API keys, wallets, hot wallets, webhooks, branding and checkout domain.
- A dashboard with roles, two-factor authentication and an audit log. The dashboard works with JavaScript off.
- A TypeScript SDK (`@turmite/sdk`) and a WooCommerce plugin.

## What Turmite does not do

- It does not run a hosted service. You run the server, the database and the wallets.
- It does not pay out in fiat money. You receive crypto in your own wallets.
- It does not send telemetry. The daily update check reads the public GitHub releases and security advisories of `loruorg/turmite`, and sends only the version. You can turn it off in the dashboard.

## Parts

Turmite is one Rust binary and a PostgreSQL database. The binary runs as two processes:

| Process | Does | Holds keys |
|---|---|---|
| `api` | Merchant API, checkout pages, dashboard | No private keys |
| `worker` | Chain watchers, confirmations, sweeps, payouts, webhook delivery, update checks | Decrypts hot keys and the deposit seed. Opens no port |

See [Architecture](/docs/architecture/).

## For coding agents

The docs are available in forms that a coding agent can read:

- [`/llms.txt`](/llms.txt): an index of the docs pages.
- [`/llms-full.txt`](/llms-full.txt): all docs pages in one file.
- Each page as Markdown: add `.md` to its path, for example [`/docs/webhooks.md`](/docs/webhooks.md).
- [`/openapi.yaml`](/openapi.yaml): the OpenAPI description of the merchant API.

Give the agent `llms-full.txt` and the task, for example: "Add Turmite payments to the checkout of this shop. Verify the IPN signature."

## Next steps

- Try it on your computer: [Quick start](/docs/quick-start/).
- Install it on a server: [Install on a server](/docs/install/).
- Connect your shop: [Accept your first payment](/docs/first-payment/).
- Move an existing integration: [Migrate from a hosted gateway](/docs/migrate/).
- Read what a server compromise can do: [Security model](/docs/security-model/).
