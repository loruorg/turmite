---
title: Install on a server
description: Install Turmite on a Linux server with the install script, set up DNS and TLS, and create the root user.
sidebarTitle: Install on a server
---

This page installs Turmite on a Linux server with Docker Compose and Caddy. Caddy gets the TLS certificate automatically. At the end you have a running server, a root user and two-factor authentication.

## Requirements

- A Linux server, `amd64` or `arm64`. The image is published for both.
- Ubuntu or Debian, if you want the script to install Docker. On other distributions, install Docker Engine and the Compose plugin first.
- `curl` and root access through `sudo`.
- A host name for the server, for example `pay.example.com`.
- Ports 80 and 443 open to the internet. Caddy uses them to get the certificate.
- For mainnet: an SMTP account. Turmite sends payout approval codes by email.

Use a server that runs nothing else. A breach of another service on the same server can reach the hot wallets.

## Choose a mode

Each install runs in one mode. You set the mode once, at install time. Turmite refuses to start when the mode in `.env` differs from the mode of the database.

| Mode | Networks | Use it to |
|---|---|---|
| `testnet` | Bitcoin Signet, Monero Stagenet, Ethereum Sepolia, Base Sepolia, Polygon Amoy, BNB Smart Chain Testnet, TRON Nile, Solana Devnet, TON Testnet | Test your integration with free test coins |
| `mainnet` | The main networks | Take real payments |

Install a testnet server first. Install a separate mainnet server when your integration works.

## 1. Point the DNS name at the server

1. Find the public IP address of the server:

   ```bash
   curl -4 https://ifconfig.me; echo
   ```

2. At your DNS provider, add an `A` record for `pay` with that address. Add an `AAAA` record if the server has IPv6.
3. Wait until the name resolves to the server:

   ```bash
   dig +short pay.example.com
   ```

To put Cloudflare in front of the server, see [Reverse proxy, Cloudflare and firewall](/docs/reverse-proxy/).

## 2. Run the installer

Run this on the server:

```bash
curl -fsSL https://turmite.com/install.sh | sudo sh -s -- --site-url https://pay.example.com
```

The script asks for the mode and for a hot-wallet passphrase. If Docker is missing, it asks before it installs Docker.

| Flag | Default | Meaning |
|---|---|---|
| `--site-url URL` | asks | The public URL of the server. It can have a path, see [Run under a path](/docs/path-prefix/) |
| `--mode MODE` | asks, `mainnet` if you press Enter | `mainnet` or `testnet` |
| `--version X.Y.Z` | the latest release | The Turmite version |
| `--dir DIR` | `/opt/turmite` | The install directory |
| `--yes` | off | Do not ask. Install Docker if it is missing, use `mainnet` if `--mode` is not given, and make a random passphrase |
| `--no-start` | off | Write the files, but do not start the stack |
| `--upgrade` | off | Upgrade an existing install. See [Upgrades](/docs/upgrades/) |
| `--local DIR` | off | Take the files from a checkout of the repository in `DIR`, not from GitHub |

Use a passphrase of 12 characters or more. `turmite init` refuses a shorter one. Write it down and store it offline. The worker needs it to decrypt the hot keys and the deposit seed. With `--yes`, the script makes a random passphrase and stores it in `secrets/hot_passphrase`.

On mainnet the site URL must start with `https://`. `turmite init` and the server refuse an `http://` site URL in mainnet mode.

## What the installer does

1. It installs Docker Engine and the Compose plugin from Docker's apt repository, if they are missing.
2. It finds the latest release, unless you give `--version`.
3. It writes `docker-compose.yml`, `Caddyfile`, `backup.sh` and `restore.sh` to `/opt/turmite`.
4. It pulls the image `loruorg/turmite:<version>` from Docker Hub.
5. It runs `turmite init` in a container. This command asks for the passphrase and writes `.env` and the `secrets/` directory.
6. It sets `TURMITE_VERSION` in `.env`.
7. It starts the stack and waits up to 2 minutes until the `api` is healthy. Migrations run when the services start.
8. It prints the next steps.

The script stops if `/opt/turmite/.env` already exists. Use `--upgrade`, or remove the directory to start again.

| Service | Runs | Port |
|---|---|---|
| `postgres` | PostgreSQL 18, on an internal network | None published |
| `api` | `turmite serve --role api` | 8080, internal only |
| `worker` | `turmite serve --role worker` | None |
| `caddy` | Caddy with automatic TLS | 80 and 443, and UDP 443 for HTTP/3 |

The `api` and the `worker` run as a non-root user, with a read-only file system and no Linux capabilities. Only the `worker` mounts `secrets/`. The volumes are `pgdata`, `caddy-data` and `caddy-config`.

## Files

| Path | Holds | Back it up |
|---|---|---|
| `docker-compose.yml` | The services | No. `--upgrade` replaces it and keeps a copy in `backups/` |
| `Caddyfile` | The Caddy configuration | Yes, if you change it. `--upgrade` keeps your file |
| `.env` | Version, mode, site URL, `TURMITE_SECRET`, `TURMITE_HOT_RECIPIENT`, the paths of the identity and passphrase files, the database password and URL, the Caddy site address. Proxies and SMTP as comments | Yes, offline. Without `TURMITE_SECRET` the sealed data in the database is unreadable |
| `secrets/hot-identity.age` | The age identity for hot keys and the deposit seed, encrypted with your passphrase | Yes, offline |
| `secrets/hot_passphrase` | The passphrase, so the worker can restart without you | See [Run without a stored passphrase](#run-without-a-stored-passphrase) |
| `backup.sh`, `restore.sh` | Database dump and restore | No. `--upgrade` replaces them |
| `backups/` | Database dumps | Copy them off the server |

All paths are in `/opt/turmite`. `turmite init` writes `.env` and the files in `secrets/` with mode `600`. It gives `secrets/` to the user of the `worker` container. Only root can read `.env`, so run `docker compose`, `backup.sh` and `restore.sh` as root.

See [Backups and restore](/docs/backups/).

## 3. Check the install

```bash
curl https://pay.example.com/healthz
```

The response is `ok`. If it is not, read the logs:

```bash
cd /opt/turmite
sudo docker compose ps
sudo docker compose logs --tail=100 api worker caddy
```

If Caddy cannot get a certificate, check the DNS record and that ports 80 and 443 are open. See [Troubleshooting](/docs/troubleshooting/).

## 4. Set up email

On mainnet, Turmite needs SMTP to send payout approval codes. Without SMTP on mainnet, an action that sends email fails with "Email is not set up. Set TURMITE_SMTP_URL." On testnet, SMTP is optional: Turmite writes each message to the log instead.

1. Open `/opt/turmite/.env` as root.
2. Set the two SMTP variables:

   ```dotenv
   TURMITE_SMTP_URL=smtps://user:password@smtp.example.com:465
   TURMITE_SMTP_FROM=Turmite <payments@example.com>
   ```

   Use `smtps://` for TLS on port 465. For STARTTLS on port 587, use `smtp://user:password@smtp.example.com:587?tls=required`. A `smtp://` URL without `?tls=required` sends mail without encryption. Percent-encode special characters in the user name and the password.

3. Apply the change:

   ```bash
   cd /opt/turmite && sudo docker compose up -d
   ```

## 5. Create the root user

1. Open `https://pay.example.com/dashboard`. With no user in the database, it opens the setup page.
2. Enter your email address and a password of 12 characters or more, two times.
3. Submit the form. This user is the root user. Turmite makes a first project, "My shop", and signs you in.

The setup page works only while the database has no users. After that, sign in at `/dashboard/login`.

Do this step immediately after the install. Until the root user exists, any visitor can open the setup page.

## 6. Enable two-factor authentication

1. Open the account page in the dashboard. The link is your email address in the sidebar.
2. Click **Set up two-factor**.
3. Scan the QR code with an authenticator app.
4. Enter the code from the app and click **Turn on**.
5. Store the key that the page shows offline, next to your password.

When two-factor is on, you need a code to sign in. You also need a fresh code for sensitive actions, for example API keys, wallets, nodes, the IPN secret and payouts. [Dashboard and roles](/docs/dashboard/#actions-that-need-a-fresh-code) lists them. On mainnet, Turmite refuses these actions until two-factor is on.

If a user loses the authenticator, an operator with shell access runs `turmite user reset2fa <email>`. See [CLI](/docs/cli/).

## 7. Configure the server

Do these steps in this order:

1. [Networks and nodes](/docs/nodes/): enable networks, add RPC nodes, check confirmations.
2. [Wallets per chain](/docs/wallets/): set where each network sends your funds.
3. Hot wallets: add one for each network that needs gas for sweeps or payouts. See [Payouts](/docs/payouts/).
4. [Projects and checkout domains](/docs/projects/): set the branding and the return URL.
5. [Accept your first payment](/docs/first-payment/): create an API key and connect your shop.
6. [Keep the dashboard private](/docs/private-dashboard/).
7. Work through the [Go-live checklist](/docs/go-live/) before you take real payments.

## Run without a stored passphrase

By default the passphrase is in `secrets/hot_passphrase`. The worker can then restart without you. To not store it in `secrets/`, run `turmite init` with `--no-store-passphrase` on a new server. On an existing server:

1. Delete the file, and remove the `TURMITE_HOT_PASSPHRASE_FILE` line from `.env`:

   ```bash
   sudo shred -u /opt/turmite/secrets/hot_passphrase
   ```

2. Give the passphrase in the environment when you start the worker. The compose file passes `TURMITE_HOT_PASSPHRASE` from your shell to the worker. It takes precedence over `TURMITE_HOT_PASSPHRASE_FILE`:

   ```bash
   sudo -i
   cd /opt/turmite
   read -rs TURMITE_HOT_PASSPHRASE && export TURMITE_HOT_PASSPHRASE
   docker compose up -d worker
   unset TURMITE_HOT_PASSPHRASE
   ```

Docker keeps the variable in the configuration of the container. Root on the server can read it with `docker inspect`. When Docker restarts the same container, the worker has the passphrase. When Compose makes a new container, for example after a change to `.env` or an upgrade, do step 2 again.

The worker does not start without the passphrase. It stops with the message "set TURMITE_HOT_PASSPHRASE or TURMITE_HOT_PASSPHRASE_FILE to unlock the hot-wallet identity". While the worker is down, Turmite does not confirm, sweep or notify payments. The `api` process continues to serve the checkout, the dashboard and the merchant API.

## Upgrade or remove

To upgrade, see [Upgrades](/docs/upgrades/).

To remove Turmite, stop the stack and delete the directory. Back up the database and the secrets first.

```bash
cd /opt/turmite && sudo docker compose down -v
sudo rm -rf /opt/turmite
```
