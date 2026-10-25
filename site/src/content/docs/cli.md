---
title: CLI
description: Reference for the turmite command and its subcommands.
---

Turmite is one binary, `turmite`. The Docker image runs it as its entrypoint. On an install made with `install.sh`, run commands as root from `/opt/turmite` through `docker compose`.

- In a running container, give the full command: `docker compose exec api turmite version`.
- In a new container, give only the subcommand, because the entrypoint is `turmite`: `docker compose run --rm api migrate`.

`turmite` with no command runs `turmite serve`. The logs are JSON lines when the output is not a terminal.

| Command | Does |
|---|---|
| `turmite serve` | Runs the server |
| `turmite init` | Writes `.env` and `secrets/` for a new install |
| `turmite migrate` | Runs the database migrations |
| `turmite user reset2fa <email>` | Turns off two-factor for one user |
| `turmite counters raise --by <n>` | After a database restore, skips deposit address indexes |
| `turmite version` | Prints the version |
| `turmite healthcheck` | Checks that the local server answers |

`turmite --help` and `turmite <command> --help` print the options.

## turmite serve

```text
turmite serve [--role all|api|worker] [--demo]
```

| Option | Meaning |
|---|---|
| `--role all` | The default. Runs the API and the worker in one process. For small installs and local tests |
| `--role api` | Merchant API, checkout pages and dashboard. Holds no private key |
| `--role worker` | Chain watchers, confirmations, expiry, sweeps, payouts, webhook delivery, address pool refill, exchange swaps and update checks. Opens no port |
| `--demo` | Uses mock chains in place of real nodes. For a local trial only. See [Quick start](/docs/quick-start/) |

The `api` role listens on `TURMITE_LISTEN` (default `0.0.0.0:8080`). Only the `worker` role can decrypt hot keys and the deposit seed. The install script runs the two roles in separate containers, so the process that faces the internet holds no key.

Migrations run when `serve` starts. They are in the binary.

| Role | Reads |
|---|---|
| `api` | `TURMITE_DATABASE_URL`, `TURMITE_SITE_URL`, `TURMITE_SECRET`, `TURMITE_MODE`, `TURMITE_HOT_RECIPIENT`, `TURMITE_LISTEN`, `TURMITE_TRUSTED_PROXIES`, `TURMITE_SMTP_URL`, `TURMITE_SMTP_FROM` |
| `worker` | `TURMITE_DATABASE_URL`, `TURMITE_SITE_URL`, `TURMITE_SECRET`, `TURMITE_MODE`, `TURMITE_HOT_IDENTITY_FILE`, `TURMITE_HOT_PASSPHRASE` or `TURMITE_HOT_PASSPHRASE_FILE`, `TURMITE_SMTP_URL`, `TURMITE_SMTP_FROM` |
| `all` | The union of both |

Start-up checks:

- Every role refuses to start without `TURMITE_SECRET`, except with `--demo`.
- Every role refuses to start on mainnet with an `http://` site URL, and when `TURMITE_MODE` differs from the mode of the database.
- The `worker` and `all` roles refuse to start when `TURMITE_HOT_IDENTITY_FILE` is set and no passphrase is given, or when the passphrase is wrong.
- The `api` role does not check `TURMITE_HOT_RECIPIENT` at start. Without it, the dashboard cannot generate or import hot keys.

With `--demo`, `TURMITE_SECRET` is optional, the mode is always `testnet`, and the `TURMITE_HOT_*` variables are not used. See [Environment variables](/docs/environment/).

Examples:

```bash
# One process, for a local test
turmite serve

# Two processes, as the install script runs them
turmite serve --role api
turmite serve --role worker

# Mock chains on your computer
turmite serve --demo
```

Run one `worker` per install. Two workers process the same payments at the same time. You can run more than one `api` process. Each `api` process counts rate limits and wrong two-factor codes on its own.

## turmite init

Writes the configuration for a new install: `.env` and the `secrets/` directory. The install script runs it for you.

```text
turmite init --site-url URL [--mode testnet|mainnet] [--dir DIR] [--no-store-passphrase] [--owner UID:GID] [--force]
```

| Option | Meaning |
|---|---|
| `--site-url` | Required. The public URL, for example `https://pay.example.com`. Mainnet needs `https://` |
| `--mode` | `testnet` (the default) or `mainnet`. The install script uses `mainnet` by default |
| `--dir` | The directory with `docker-compose.yml`. The default is the current directory. In the image it is `/data` |
| `--no-store-passphrase` | Do not write the passphrase to disk. You then give `TURMITE_HOT_PASSPHRASE` at start |
| `--owner` | When `init` runs as root, give `secrets/` and its files to this user and group. The default is `65532:65532`, the user of the image |
| `--force` | Replace an existing `.env` and identity. The old hot keys then become unreadable |

It does these steps:

1. It asks for a hot-wallet passphrase of at least 12 characters, two times. It uses `TURMITE_HOT_PASSPHRASE` instead when that variable is set. Without a terminal and without the variable, it stops.
2. It makes an age identity, encrypts it with the passphrase and writes it to `secrets/hot-identity.age`.
3. It writes the passphrase to `secrets/hot_passphrase`, so the worker can restart without you. `--no-store-passphrase` skips this step.
4. It sets mode `700` on `secrets/`. As root, it gives `secrets/` to the `--owner` user.
5. It writes `.env` with the version, the mode, the site URL, a random `TURMITE_SECRET`, the public key of the identity as `TURMITE_HOT_RECIPIENT`, the file paths for the worker, a random database password, `TURMITE_DATABASE_URL` and the Caddy site address.

The files get mode `600`. `turmite init` refuses to overwrite an existing `.env` or identity without `--force`. Keep a copy of `.env` and `secrets/` offline. See [Backups and restore](/docs/backups/).

To run it from the image:

```bash
sudo docker run --rm -it --user 0:0 -v /opt/turmite:/data loruorg/turmite:0.1.0 \
  init --mode mainnet --site-url https://pay.example.com
```

## turmite migrate

Runs the database migrations, prints `migrations applied` and exits. Needs `TURMITE_DATABASE_URL` and `TURMITE_SECRET`.

`turmite serve` runs the migrations at start, so you do not need this command in normal use. Use it to migrate before you start a new version:

```bash
cd /opt/turmite
sudo docker compose run --rm api migrate
```

Take a database dump first. See [Upgrades](/docs/upgrades/).

## turmite user reset2fa

```text
turmite user reset2fa <email>
```

Turns off two-factor authentication for the user with this email. The user then signs in with the password and enrols again on the account page. Needs `TURMITE_DATABASE_URL` and `TURMITE_SECRET`.

```bash
cd /opt/turmite
sudo docker compose exec api turmite user reset2fa ops@example.com
```

The command also ends all sessions of the user and writes a `user.2fa_reset` entry to the audit log.

Use it when a user loses the authenticator. Check the identity of the person before you run it. Anyone who can run this command has shell access to the server. The root user can also reset two-factor for another user on the **Users** page.

## turmite counters raise

```text
turmite counters raise --by <n>
```

Use it after you restore a database dump. It adds `n` to each deposit address counter (per network, per EVM family and per project wallet) and deletes the free addresses of the address pool. New payments then do not get the addresses that Turmite gave out after the dump. The worker makes new pool addresses. `n` is 1 to 1000000. Needs `TURMITE_DATABASE_URL` and `TURMITE_SECRET`.

Use an `n` larger than the number of payments made after the dump. On Bitcoin, the skipped addresses are a gap in your wallet: set the gap limit of your wallet above `n`. See [Backups and restore](/docs/backups/#after-a-restore).

The command writes a `counters.raise` entry to the audit log.

## turmite version

Prints `turmite` and the version, for example `turmite 0.1.0`, and exits.

```bash
sudo docker compose exec api turmite version
```

The dashboard shows the same version on the **Updates** page.

## turmite healthcheck

Sends `GET /healthz` to the server on `TURMITE_LISTEN`, through the loopback address. It exits with `0` when the answer is `200` and `ok`, and with `1` in other cases. The image has no `curl`, so its Docker `HEALTHCHECK` runs this command every 30 seconds.

```bash
sudo docker compose exec api turmite healthcheck
```
