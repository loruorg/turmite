---
title: Contributing
description: Build Turmite from source, run the tests and send a pull request.
---

Turmite is a community project. The code, the issues and the pull requests are on [GitHub](https://github.com/loruorg/turmite). Contributions are under the MIT license.

Report security problems in private. Do not open a public issue. See [Reporting a vulnerability](/docs/report-vulnerability/).

## Repository layout

| Path | Contents |
|---|---|
| `crates/turmite/` | The library and the `turmite` binary (Rust 2024) |
| `crates/turmite/src/http/` | Merchant API, checkout, dashboard, assets, middleware |
| `crates/turmite/src/chains/` | One driver per chain family, the mock driver, the network catalog, node health and failover |
| `crates/turmite/src/worker/` | Background loops: chain watching, sweeps, the EVM sweeper, the address pool |
| `crates/turmite/src/payouts/` | Payouts: limits, approvals, signing per chain |
| `crates/turmite/src/exchange/` | Pay with any coin through Trocador |
| `crates/turmite/src/db/` | Database access, one file per area |
| `crates/turmite/templates/` | Server-rendered templates: dashboard and checkout |
| `crates/turmite/migrations/` | Database migrations |
| `contracts/` | Foundry project: EVM `DepositFactory` and `DepositWallet` |
| `sdk/typescript/` | `@turmite/sdk` |
| `plugins/woocommerce/` | The WooCommerce plugin |
| `deploy/` | Compose file, Caddyfile, `install.sh`, Kubernetes manifests, testnet stack |
| `site/` | This website |
| `openapi.yaml` | OpenAPI 3.1 description of the merchant API and the webhook payloads |

## Requirements

- Rust 1.99.1. `rust-toolchain.toml` selects it when you use `rustup`.
- Docker with the Compose plugin, for PostgreSQL 18 and the testnet stack.
- Foundry, only to change the EVM contracts.
- Node.js 26 and pnpm 12, only for the SDK and the site. CI uses these versions.

## Build and run

1. Clone the repository:

   ```bash
   git clone https://github.com/loruorg/turmite.git
   cd turmite
   ```

2. Start PostgreSQL. It listens on `127.0.0.1:5433`:

   ```bash
   docker compose -f deploy/testnets/docker-compose.yml up -d postgres
   ```

3. Run the server with mock chains:

   ```bash
   cargo run -- serve --demo
   ```

   The defaults of `TURMITE_DATABASE_URL` and `TURMITE_SITE_URL` point to this database and to `http://localhost:8080`. With `--demo`, Turmite makes a random `TURMITE_SECRET` when none is set.

4. Open `http://localhost:8080/dashboard` and create the first user.

## Tests

```bash
cargo test
```

The tests need the PostgreSQL server from step 2. Each test makes its own database there. To use another server, set `TURMITE_TEST_PG=postgres://user:pass@host:port`. The default features build every chain, so the tests need no feature flag.

The real-chain tests skip unless `TURMITE_TEST_CHAINS` names their chain (`bitcoin,monero`) or is `all`. They need the [local testnet stack](#local-testnet-stack).

Before you send a pull request, also run:

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
```

For the EVM contracts:

```bash
cd contracts && forge test
```

Do not change the deployed contract bytecode without a migration plan. The factory bytecode decides every EVM deposit address.

## Local testnet stack

`deploy/testnets` runs PostgreSQL and a local node for each chain. All images run on `arm64` and `amd64`.

```bash
docker compose -f deploy/testnets/docker-compose.yml up -d                # PostgreSQL, Bitcoin, Monero, EVM
docker compose -f deploy/testnets/docker-compose.yml --profile all up -d  # adds Solana, Tron, TON

deploy/testnets/verify.sh     # checks each local node
scripts/e2e.sh                # every test, with the real-chain tests on every chain
```

`scripts/e2e.sh` fails when a real-chain test skips. The profile `public` adds clients for the public testnets (Bitcoin Signet, Monero Stagenet). Optional settings, such as API keys for `verify.sh --public`, go in `deploy/testnets/.env`. See `.env.example`. `deploy/testnets/README.md` lists the ports, the funding commands and the faucets.

## Pull requests

- Keep each pull request to one change.
- Add or update tests for the change.
- Update the docs in `site/src/content/docs/` when behaviour changes.
- Write user-facing text in ASD-STE100 Simplified Technical English: short sentences, active voice, one fact per sentence.
- Do not add telemetry, analytics or third-party trackers.
- Do not add an environment variable that turns a feature on or off. Settings belong in the dashboard. Open an issue first if you think a new variable is necessary.

## The website

The site is an Astro project in `site/`:

```bash
cd site
pnpm install
pnpm dev
```

`pnpm build` writes the static site to `site/dist/` and builds the search index.
