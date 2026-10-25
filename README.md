# Turmite

Self-hosted crypto payments. You run the server, the coins go to your own wallets, and no third party holds funds or sees your orders.

- Payments and invoices with a hosted checkout that works without JavaScript.
- Bitcoin (watch-only xpub), Monero (view-only wallet), Ethereum, Base, Polygon, BNB Smart Chain, Tron, Solana and TON, with USDT and USDC where the chain has them.
- Signed webhooks, payouts from hot wallets with approvals, projects, team roles and two-factor sign-in.
- Customers can pay with other coins through an exchange that you connect.
- Merchant API compatible with the common hosted-gateway API: to move a shop, change the base URL, the API key and the IPN secret.
- TypeScript SDK and a WooCommerce plugin.

## Start

```sh
curl -fsSL https://turmite.com/install.sh | sudo sh -s -- --site-url https://pay.example.com
```

Read [Install on a server](https://turmite.com/docs/install/) first. To try it on your computer with mock chains, see [Quick start](https://turmite.com/docs/quick-start/).

## Repository

| Path | Content |
| --- | --- |
| `crates/turmite` | Server: API, checkout, dashboard and chain watcher (Rust) |
| `contracts` | EVM deposit contracts (Solidity, Foundry) |
| `sdk/typescript` | `@turmite/sdk` |
| `plugins/woocommerce` | WooCommerce plugin |
| `deploy` | Docker Compose, Caddy, Kubernetes and local testnets |
| `site` | turmite.com and the docs |
| `openapi.yaml` | API reference |

## Develop

Requirements: Rust (version in `rust-toolchain.toml`), Docker, PostgreSQL 18.

```sh
docker compose -f deploy/testnets/docker-compose.yml up -d postgres
cargo test
```

Tests use PostgreSQL on `127.0.0.1:5433`. To use another server, set `TURMITE_TEST_PG=postgres://user:pass@host:port`.

See [Contributing](https://turmite.com/docs/contributing/).

## License

[MIT](LICENSE). Security reports: [SECURITY.md](SECURITY.md).
