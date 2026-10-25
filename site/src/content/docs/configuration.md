---
title: Configuration
description: Set secrets and per-deployment values in the environment, and set everything else in the dashboard.
---

Turmite reads a short list of environment variables at start. Every other setting is in the database, and you edit it in the dashboard.

## Where settings live

| Kind | Where | Who edits it |
|---|---|---|
| Secrets and per-deployment values | Environment (`/opt/turmite/.env`) | The server operator |
| Networks, RPC nodes, confirmations, tokens | Database | Root and admin users, in **Server → Networks** |
| Hot wallets, users, update checks | Database | Root and admin users |
| Checkout domain of a project | Database | Root and admin users |
| Wallets, API keys, payouts, branding, exchange | Database, per project | Project admins |
| Webhook endpoints | Database, per project | Developers and project admins |

`turmite init` writes the `.env` file and the `secrets/` folder. The install script runs it for you. See [Install on a server](/docs/install/).

## Environment variables

[Environment variables](/docs/environment/) lists each variable with its default. Turmite has no variable that turns a feature on or off.

`turmite init` writes this file. The install script then sets `TURMITE_VERSION` to the release that it installs.

```dotenv
# Written by `turmite init`. Keep a copy offline with secrets/.
# Image tag that docker compose runs. install.sh --upgrade changes it.
TURMITE_VERSION=0.1.0
TURMITE_MODE=mainnet
TURMITE_SITE_URL=https://pay.example.com
TURMITE_SECRET=3f9c...64 hex characters...a1
TURMITE_HOT_RECIPIENT=age1qz...
TURMITE_HOT_IDENTITY_FILE=/secrets/hot-identity.age
TURMITE_HOT_PASSPHRASE_FILE=/secrets/hot_passphrase
POSTGRES_PASSWORD=0b6c...32 hex characters...
TURMITE_DATABASE_URL=postgres://turmite:0b6c...@postgres:5432/turmite
# Caddy site address: the host name gets automatic TLS; http://:80 is plain HTTP.
TURMITE_CADDY_ADDRESS=pay.example.com
# Proxies in front of Caddy (for example Cloudflare), with the private ranges, space-separated.
# TURMITE_TRUSTED_PROXIES=10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 173.245.48.0/20 ...
# Optional: email for payout approvals and security notices.
# TURMITE_SMTP_URL=smtps://user:password@smtp.example.com:465
# TURMITE_SMTP_FROM=Turmite <pay@example.com>
```

`TURMITE_VERSION`, `POSTGRES_PASSWORD` and `TURMITE_CADDY_ADDRESS` are for Docker Compose and Caddy. `TURMITE_CADDY_ADDRESS` is the host name of an `https://` site URL. With a path in the site URL, with `http://`, or with a `localhost`, `.localhost` or `.local` host, it is `http://:80`.

The compose file gives each service only the variables that it needs. The `worker` gets the identity and passphrase variables and mounts `secrets/`. The `api` gets `TURMITE_HOT_RECIPIENT` and `TURMITE_TRUSTED_PROXIES`. It never receives the identity or the passphrase.

## Site URL

- Use the URL that customers see. Checkout links and the merchant API base URL come from it.
- Use `https://` on mainnet. Turmite refuses to start on mainnet with an `http://` site URL.
- Session cookies get the `Secure` flag on `https://` URLs.
- A path is allowed. See [Run under a path](/docs/path-prefix/).
- Dashboard forms must carry an `Origin` (or `Referer`) header that matches the origin of this URL.

## Mode

`TURMITE_MODE` selects the networks that Turmite seeds on first start. A `testnet` install uses Bitcoin Signet, Monero Stagenet, Ethereum Sepolia, Base Sepolia, Polygon Amoy, BNB Smart Chain Testnet, TRON Nile, Solana Devnet and TON Testnet. A `mainnet` install uses the main networks. See [Networks and nodes](/docs/nodes/).

Turmite stores the mode in the database. It refuses to start when `TURMITE_MODE` differs from the stored mode. To go from testnet to mainnet, make a new install with a new database.

## Hot wallet keys

Turmite uses [age](https://age-encryption.org) to protect hot keys and the deposit seed.

- The `api` has only the public key (`TURMITE_HOT_RECIPIENT`). It can encrypt keys. It cannot decrypt them.
- The `worker` has the identity file and the passphrase. It is the only process that can sign.

By default the passphrase is stored in `secrets/hot_passphrase`, so the worker can restart without you. To give it at each start instead, see [Run without a stored passphrase](/docs/install/#run-without-a-stored-passphrase).

## Email

Turmite sends payout approval codes and security advisory notices by email. On mainnet, set `TURMITE_SMTP_URL` and `TURMITE_SMTP_FROM` before you approve a payout. Any SMTP provider works. See [TURMITE_SMTP_URL](/docs/environment/#turmite_smtp_url).

On testnet, SMTP is optional. Without it, Turmite writes each email to the log, with a warning. On mainnet without SMTP, Turmite starts, but each approval code fails.

## Settings in the dashboard

These settings are in the database. They do not need a restart.

| Setting | Page | Scope |
|---|---|---|
| Project name, payment window, invoice window | Settings → General | Project |
| IPN signature header, IPN secret | Settings → General | Project |
| Logo, theme, accent colour, message, support links, return URL, allowed return hosts | Settings → Checkout | Project |
| Trocador API key, markup, lowest KYC rating, allowed coins | Settings → Exchange | Project |
| Checkout domain | Settings → Domain | Project |
| Wallets per network | Wallets | Project |
| API keys | API keys | Project |
| Webhook endpoints | Webhooks | Project |
| Payout limits | Payouts → Limits | Project |
| Address book | Payouts → Address book | Project |
| Members and project roles | Team | Project |
| Networks, nodes, confirmations, tokens | Server → Networks | Install |
| Hot wallets | Server → Hot wallets | Install |
| Users | Server → Users | Install |
| Audit log | Server → Audit log | Install |
| Update checks | Server → Updates | Install |

The **Server** section of the menu shows only for root and admin users.

Settings that move or redirect money need a fresh TOTP code. See [Dashboard and roles](/docs/dashboard/#two-factor-authentication).
