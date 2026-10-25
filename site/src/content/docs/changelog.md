---
title: Changelog
description: Changes in each Turmite release.
---

Each release is on [GitHub Releases](https://github.com/loruorg/turmite/releases). The image is `ghcr.io/loruorg/turmite:<version>`. Security fixes are also published as [security advisories](https://github.com/loruorg/turmite/security/advisories).

## 0.1.0

The first public release.

- Merchant API with the paths, fields, statuses and IPN signature of the common hosted-gateway API.
- Networks: Bitcoin, Monero, Ethereum, Base, Polygon, BNB Smart Chain, Tron, Solana and TON, with USDC and USDT where listed.
- Image for `linux/amd64` and `linux/arm64`.
- Watch-only Bitcoin from a BIP84 xpub. View-only Monero from an address and a private view key.
- CREATE2 deposit contracts on the EVM networks, locked to the cold wallet in the project's factory.
- Per-payment keys on Tron, Solana and TON, swept to the cold wallet.
- Checkout pages for invoices and payments that work with JavaScript off.
- Per-payment IPN callbacks and project webhook endpoints. Each delivery has the compatible HMAC-SHA512 signature and a timestamped HMAC-SHA256 signature.
- Payouts from hot wallets with an address book, limits and approval by TOTP and an emailed code.
- Optional payment with any coin that Trocador lists, delivered to the payment's own deposit address.
- Server-rendered dashboard with roles, two-factor authentication and an audit log.
- Projects with branding, checkout domains and a return URL.
- Site URL with a path prefix.
- Daily update checks against GitHub releases and advisories.
- RPC node health checks and failover.
- Install script with Docker Compose and Caddy, with a database dump before each upgrade. `turmite init` for new installs. Backup and restore scripts.
- TypeScript SDK `@turmite/sdk` and a WooCommerce plugin.
