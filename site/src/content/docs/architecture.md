---
title: Architecture
description: How the Turmite binary, its two roles and PostgreSQL work together, and how a payment moves through them.
---

Turmite is one Rust binary. It runs in two roles, `api` and `worker`, that share one PostgreSQL database. The roles never call each other. Caddy terminates TLS in front of the `api` role.

## Overview

```text
 customer browser --+
                    +--> Caddy (TLS) --> api --+
 your shop server --+                          |
                                               +--> PostgreSQL
                                               |
                                    worker ----+
                                      |
                                      +--> RPC nodes, webhook endpoints,
                                           Trocador, SMTP, GitHub
```

The `api` role answers HTTP requests. The `worker` role does everything that is slow, periodic or needs a private key. PostgreSQL is the only state they share.

Both roles make outbound requests:

| Target | `api` | `worker` |
|---|---|---|
| RPC nodes | Address checks, deposit addresses on Bitcoin, Monero and EVM, node probes every 120 seconds | Chain watching, sweeps, payouts, address pool, node probes every 30 seconds |
| CoinGecko, Coinbase | Prices for quotes and payout limits | Prices for EVM sweep batches |
| Trocador | Starts swaps. Fetches a swap when Trocador reports a change | Polls open swaps every 10 seconds. Refreshes the coin list once a day |
| SMTP server | Payout approval codes | Security advisory emails |
| GitHub | **Check now** on the Updates page | The daily update check |
| Webhook endpoints | None | Every delivery |
| Logo URLs | One download when an admin saves a logo URL | None |

## Roles

Start a role with `turmite serve --role all|api|worker`. The default is `all`, which runs both roles in one process. The Docker Compose stack runs `api` and `worker` as separate containers.

| Role | Does | Holds a private key | Opens a port |
|---|---|---|---|
| `api` | Merchant API, checkout pages, dashboard. Gives each payment a deposit address. Encrypts imported or generated hot keys to `TURMITE_HOT_RECIPIENT` | No | Yes, `TURMITE_LISTEN` (default `0.0.0.0:8080`) |
| `worker` | Chain watchers, confirmations, expiry, sweeps, payouts, webhook delivery, address pool refill, swap status, update checks | Yes. It is the only process that can decrypt hot keys and the deposit seed | No |

The worker loads the hot-wallet identity at start. It does not start when `TURMITE_HOT_IDENTITY_FILE` is set and no passphrase is given.

| Loop in the worker | Interval |
|---|---|
| Check every open payment, then send due webhooks | 5 seconds |
| Sign and track payouts | 5 seconds |
| Poll open swaps | 10 seconds |
| Refill the address pool | 30 seconds |
| EVM batch sweeps | 30 seconds |
| Probe the RPC nodes | 30 seconds |
| Delete expired sessions | 1 hour |
| Update check | 1 minute after start, then once a day |

Run exactly one `worker` per install, also during an upgrade. Each `api` process keeps its rate limits and its two-factor lockout in its own memory.

See [Security model](/docs/security-model/) for why the split matters.

## Source layout

| Path | Contents |
|---|---|
| `crates/turmite/src/main.rs` | CLI entry point: `serve`, `init`, `migrate`, `user reset2fa`, `version`, `healthcheck` |
| `crates/turmite/src/init.rs` | `turmite init`: `.env` and `secrets/` |
| `crates/turmite/src/http/` | Merchant API, payout API, checkout, dashboard, static assets, middleware |
| `crates/turmite/src/db/` | PostgreSQL access, one file per area |
| `crates/turmite/src/chains/` | One driver per chain family, the node registry and failover, the EVM factory, the network catalog (`catalog.rs`) |
| `crates/turmite/src/worker/` | The background loops: chain watching, confirmations, sweeps, EVM batch sweeps, address pool refill |
| `crates/turmite/src/payouts/` | Payout limits, approval codes, the signer loop, one module per chain family |
| `crates/turmite/src/exchange/` | The Trocador client and swaps |
| `crates/turmite/src/webhooks.rs` | Webhook queue, signatures and delivery |
| `crates/turmite/src/auth/` | Passwords, two-factor codes, payout API tokens, rate limits |
| `crates/turmite/src/crypto.rs` | Sealing with `TURMITE_SECRET`, age encryption |
| `crates/turmite/src/hot.rs` | Hot wallets, encrypted with age |
| `crates/turmite/src/netguard.rs` | Outbound HTTP that refuses private addresses on mainnet |
| `crates/turmite/src/reducer.rs` | Payment status from the deposits |
| `crates/turmite/src/rates.rs` | Exchange rates: CoinGecko first, Coinbase as fallback, cached for 60 seconds |
| `crates/turmite/src/updates.rs` | Update checks |
| `crates/turmite/src/branding.rs` | Logos, colours, return hosts |
| `crates/turmite/src/mail.rs` | Email over SMTP |
| `crates/turmite/templates/` | Server-rendered HTML for the dashboard and the checkout |
| `crates/turmite/assets/` | Static files embedded in the binary, served at `/assets/` |
| `crates/turmite/migrations/` | SQL migrations |
| `contracts/` | Foundry project: EVM `DepositFactory` and `DepositWallet` |
| `sdk/typescript/` | `@turmite/sdk` |
| `plugins/woocommerce/` | WooCommerce gateway plugin |
| `deploy/` | Docker Compose, Caddyfile, `install.sh`, Kubernetes manifest, local testnet stack |
| `scripts/` | `backup.sh`, `restore.sh`, `e2e.sh` |
| `openapi.yaml` | OpenAPI 3.1 description of the merchant API and webhook payloads |

## Data model

The main tables, simplified:

| Table | Holds |
|---|---|
| `settings` | Install settings: the deposit seed (age-encrypted to `TURMITE_HOT_RECIPIENT`), the update check state, the nodes in use |
| `users`, `sessions` | Dashboard users with a global role, and sessions stored as SHA-256 hashes |
| `projects`, `project_members` | One row per shop: name, sealed IPN secret, signature header, payment and invoice windows, branding, return URL and hosts, checkout domain, exchange settings with the sealed Trocador key. Members and their project role |
| `api_keys` | Project API keys, stored as SHA-256 hashes with the last four characters |
| `audit_log` | Every sign-in and sensitive action |
| `asset`, `network`, `asset_network` | Coins, networks (confirmations, enabled, last block) and tickers with decimals and token contracts |
| `nodes` | RPC nodes with sealed URLs, a display URL without credentials, a priority and health |
| `project_wallets` | Per project and network: address and memo, Bitcoin account key and check addresses, sealed Monero view key and restore height, EVM factory |
| `deposit_addresses`, `network_address_counter`, `family_address_counter` | Deposit addresses with their derivation index, the address pool, and the next index per network and per family |
| `evm_deposits`, `evm_scans`, `evm_txs` | The factory and cold wallet of each EVM deposit address, its watch state, and the deploy and flush transactions |
| `invoices`, `payments` | Invoices and payments with amounts, status, settlement, deposit address and expiry |
| `payment_transactions` | Every incoming transfer, with transaction, block and confirmations |
| `sweeps` | Transfers from deposit addresses to the cold wallet |
| `hot_wallets`, `project_hot_wallets` | age-encrypted hot keys, and the hot wallet of each project per family |
| `payout_settings`, `payout_addresses` | Payout limits and the address book of each project |
| `payout_batches`, `payouts`, `payout_attempts`, `payout_approval_codes` | Payout requests, their signed transactions, and the emailed approval codes as keyed hashes |
| `webhook_endpoints`, `webhook_deliveries` | Endpoints with sealed secrets, and the delivery queue |
| `exchange_coins`, `exchange_trades` | The coins Trocador lists, and the swaps customers started |

## Payment lifecycle

1. Your shop calls `POST /v1/payment`, or the customer picks a coin on an invoice page.
2. The `api` role gives the payment a deposit address. See [Address pool](#address-pool).
3. The checkout page shows the amount, the address and a QR code. The status is `waiting`.
4. The worker watches the deposit address through the network's RPC nodes. When it sees a transfer, the status becomes `confirming`.
5. When the transfers with the network's required confirmations cover at least 99 % of the amount, the status becomes `confirmed`. In the same pass the worker credits the funds and the status becomes `finished`.
6. When all transfers are confirmed and cover less than 99 %, the status is `partially_paid`. A top-up can still finish the payment.
7. A payment with no deposit before its expiry becomes `expired`. The worker watches the address for one more day, so a late deposit still counts. A payment that is still underpaid one day after its expiry becomes `expired`.
8. On networks with a sweep, the worker then moves the funds to the cold wallet. The sweep does not change the payment status. Turmite never sets `sending`.
9. Each status change queues a webhook delivery.

With the exchange option, the customer pays an exchange through Trocador. The `api` role starts the swap. The exchange sends the payment coin to the same deposit address, and the steps from 4 continue as above. See [Pay with any coin](/docs/exchange/).

See [Payment statuses](/docs/statuses/) for every status.

## Address pool

The `api` role derives a deposit address itself when it needs no private key for it. On Tron, Solana and TON the address comes from a key of the install's deposit seed. Only the worker holds that seed, so the worker keeps a pool of 25 free addresses per network, and the `api` role takes one from the pool. When the pool is empty, the API answers 503 with `Retry-After: 30`. With `--role all`, the one process derives every address directly.

| Family | Deposit address |
|---|---|
| Bitcoin | Next BIP84 receive address of the project's account key, from a watch-only wallet `turmite-p<project_id>` on bitcoind |
| Monero | New subaddress of the project's view-only wallet in monero-wallet-rpc. Turmite checks the view key against the address itself |
| EVM | CREATE2 address computed from the project's factory and an install-wide salt. The contract is deployed at the first sweep |
| Tron, Solana, TON | Next key from the install's deposit seed, from the pool. The private key stays with the worker |

## Sweeps

| Family | Sweep |
|---|---|
| Bitcoin, Monero | None. Funds arrive in your own wallet |
| EVM | The worker sends one `flush` per project, network and factory from the project's EVM hot wallet. It deploys the factory first when the chain has no code at its address. The factory deploys missing deposit contracts, and they send to the cold wallet fixed in the factory. The hot wallet pays the gas |
| Tron | The worker signs a transfer with the deposit key. For TRC-20 the hot wallet first sends the TRX that the transfer burns. A TRX deposit pays its own fee |
| Solana | The worker signs a transfer with the deposit key. A SOL deposit pays its own fee. For SPL tokens the hot wallet pays the fee and creates the cold wallet's token account if needed. The empty deposit token account is closed and its rent goes to the hot wallet |
| TON | The first sweep deploys the wallet v4r2 contract at the deposit address and sends the whole balance. A settlement memo goes as a text comment |

An EVM batch goes out when gas is at most 2 % of its value, when it is worth 1000 USD or more, when it has 150 deposits, or when its oldest payment waited 6 hours. It never goes out while gas is more than 20 % of its value. A failed EVM flush is tried again after one hour.

On Tron, Solana and TON the worker sweeps each payment on its own, after it credits the payment. A sweep that fails before the broadcast is tried again on the next pass. The worker then checks each broadcast sweep on chain until it is final. A sweep that failed on chain, or that the chain does not have 10 minutes after the broadcast, gives its amount back, and the worker sends it again: at once after each of the first 3 failures, then once per hour. A sweep that can still land keeps its amount, so the worker never sends the same funds twice. After the watch window of a payment ends, the worker still checks its open sweeps and sweeps funds that stay at the deposit address, every 60 seconds.

Only the worker signs sweeps. Without a hot wallet, or without gas in it, EVM, TRC-20 and SPL sweeps stop and the funds stay at the deposit addresses.

## Nodes and failover

Each network has a list of RPC nodes, edited under **Server → Networks**. Node URLs are sealed with `TURMITE_SECRET`. The dashboard and the audit log show them without credentials.

At start, each process connects to the first node in list order that answers. The worker probes every enabled node every 30 seconds, the `api` every 120 seconds. When the node in use fails two probes in a row, or falls too far behind the best node, the process moves the network to the next healthy node. After two good probes of a node higher in the list, it moves back. The worker records each move in the audit log as `node.failover`. Both processes connect again when the node list changes.

The Networks page shows each node's status, height, lag, last check and last error, the node in use, and the last block of each network. See [Networks and nodes](/docs/nodes/).

## Webhook delivery

Each event becomes a row in `webhook_deliveries`. The worker sends due rows, signs each attempt and records the response. Failed attempts are retried on a fixed schedule. After the last attempt, the delivery is `dead`. See [Webhooks](/docs/webhooks/).

## Payouts

The `api` role stores a payout request and checks it against the limits and the address book. The worker signs approved payouts with the hot wallet key, stores each signed transaction, then broadcasts it and tracks it to a final status. EVM payouts and EVM sweeps from the same hot wallet share one nonce lock. See [Payouts](/docs/payouts/).

## Update checks

A minute after start and then once a day, the worker reads the latest release and the security advisories of `loruorg/turmite` on GitHub. The result is stored in `settings`. The dashboard shows a banner to admins when a newer release or an advisory exists. The check is on by default. Turn it off under **Server → Updates**.

## Migrations

`turmite migrate` applies the SQL migrations and exits. `turmite serve` applies them at start too, in every role.
