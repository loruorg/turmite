---
title: Turmite 0.1.0
description: The first release of Turmite, a free and self-hosted crypto payment gateway.
date: 2026-10-24
---

Turmite 0.1.0 is the first public release. Turmite is a payment gateway that you run on your own server. It is free and open source under the MIT license. There is no hosted service, no fee and no account.

## What is in this release

- One Rust binary with two roles: `api` and `worker`. PostgreSQL holds all state.
- Nine networks: Bitcoin, Monero, Ethereum, Base, Polygon, BNB Smart Chain, Tron, Solana and TON. USDC and USDT are available where the network has them.
- A merchant API with the same paths, fields, statuses and IPN signature as the common hosted-gateway API. To move an existing integration, change the base URL, the API key and the IPN secret.
- Signed webhooks, with a second timestamped signature. Turmite retries a final event for about three and a half days.
- Checkout and dashboard pages that work with JavaScript off.
- Payouts from hot wallets, with an address book, limits and approval with two-factor and an emailed code.
- Pay with any coin: connect your own Trocador API key, and customers can pay in the coins that Trocador lists.
- Projects with their own branding, API keys, wallets and checkout domains.
- An install script for a Linux server, with Caddy for TLS and a database dump before each upgrade.

## Custody

Turmite keeps keys off the server where the chain allows it. On Bitcoin it watches addresses from your xpub. On Monero it reads payments with your view key. On the EVM networks each payment has a deposit contract that can only send to your cold wallet. On Tron, Solana and TON the worker sweeps each deposit to your cold wallet. The [security model](/docs/security-model/) lists what a server compromise can and cannot do on each chain.

## Install

```bash
curl -fsSL https://turmite.com/install.sh | sudo sh -s -- --site-url https://pay.example.com
```

Read [Install on a server](/docs/install/) first. For a local trial, use the [quick start](/docs/quick-start/).

## Report problems

Open an issue on [GitHub](https://github.com/loruorg/turmite/issues). Report security problems in private, as the [vulnerability policy](/docs/report-vulnerability/) describes.
