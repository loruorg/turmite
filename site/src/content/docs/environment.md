---
title: Environment variables
description: The complete list of environment variables that Turmite reads.
---

Turmite reads a short list of environment variables. They hold secrets and values that differ between installs. All other settings live in the database. You edit them in the dashboard. See [Configuration](/docs/configuration/).

On an install made with `install.sh`, the variables are in `/opt/turmite/.env`. After you change the file, run `sudo docker compose up -d` in that directory.

## All variables

| Variable | Read by | Default | Purpose |
|---|---|---|---|
| `TURMITE_DATABASE_URL` | api, worker | `postgres://turmite:turmite@127.0.0.1:5433/turmite` | PostgreSQL URL |
| `TURMITE_SITE_URL` | api, worker | `http://localhost:8080` | Public URL. Can have a path |
| `TURMITE_SECRET` | api, worker | None. Required, except with `--demo` | 64 hex characters. Seals stored secrets |
| `TURMITE_MODE` | api, worker | `testnet` | `mainnet` or `testnet` |
| `TURMITE_LISTEN` | api | `0.0.0.0:8080` | Bind address |
| `TURMITE_TRUSTED_PROXIES` | api, Caddy | Private ranges | Proxies whose `X-Forwarded-For` is trusted |
| `TURMITE_HOT_RECIPIENT` | api | None | age public key. The api encrypts hot keys to it |
| `TURMITE_HOT_IDENTITY_FILE` | worker | None | Path to the age identity, encrypted with the passphrase |
| `TURMITE_HOT_PASSPHRASE` | worker | None | The passphrase for the identity |
| `TURMITE_HOT_PASSPHRASE_FILE` | worker | None | A file that holds the passphrase |
| `TURMITE_SMTP_URL` | api, worker | None | SMTP server URL with credentials. Needed on mainnet |
| `TURMITE_SMTP_FROM` | api, worker | `turmite@localhost` | Sender address |

With `turmite serve --role all`, one process reads all of them. An empty value counts as not set. The defaults suit a local test. On a server, `turmite init` writes every value that the server needs.

`RUST_LOG` sets the log filter. The default is `info,sqlx=warn,tower_http=warn`. The compose file does not pass it.

Docker Compose and Caddy read three more values from `.env`. Turmite does not read them:

| Variable | Purpose |
|---|---|
| `TURMITE_VERSION` | The image tag, for example `0.1.0`. `install.sh --upgrade` changes it |
| `POSTGRES_PASSWORD` | The password of the `turmite` database user |
| `TURMITE_CADDY_ADDRESS` | The Caddy site address: the host name of the site URL, or `http://:80` for plain HTTP |

## Details

### TURMITE_DATABASE_URL

The PostgreSQL connection URL.

```dotenv
TURMITE_DATABASE_URL=postgres://turmite:PASSWORD@postgres:5432/turmite
```

Percent-encode special characters in the password. `turmite init` makes a random hex password.

### TURMITE_SITE_URL

The public URL of the server. Turmite uses it for checkout links, the session cookie and the `Origin` check on dashboard forms.

```dotenv
TURMITE_SITE_URL=https://pay.example.com
```

The URL can have a path, for example `https://example.com/pay`. Turmite then serves everything under that path. See [Run under a path](/docs/path-prefix/).

On mainnet, Turmite refuses to start with an `http://` site URL. Turmite removes a trailing slash. The URL must not have a query or a fragment.

### TURMITE_SECRET

64 hex characters (32 bytes). `turmite init` makes one. To make one by hand:

```bash
openssl rand -hex 32
```

It seals the TOTP secrets, the IPN secrets, the webhook endpoint secrets, the node URLs, the Monero view keys and the exchange API keys in the database. It also signs the step between password and code at sign-in, the payout approval codes and the tokens of `POST /v1/auth`.

Keep a copy offline and keep the same value for the life of the install. Turmite has no command to change it. With another value, the sealed data is unreadable: users with two-factor cannot sign in until an operator runs `turmite user reset2fa`, and you must enter nodes, view keys, webhook endpoints and exchange keys again and rotate the IPN secrets.

### TURMITE_MODE

`mainnet` or `testnet`. The mode decides which networks Turmite seeds in the database. Turmite stores the mode in the database at the first start. If `TURMITE_MODE` differs later, Turmite refuses to start: "this database was set up for testnet; TURMITE_MODE is mainnet. Use a new database for the other mode."

### TURMITE_LISTEN

The address and port that the `api` process binds to. Default `0.0.0.0:8080`. `turmite healthcheck` connects to the same port. The compose file does not pass this variable, so the `api` container listens on 8080.

### TURMITE_TRUSTED_PROXIES

The proxies whose `X-Forwarded-For` header Turmite trusts. Turmite takes the client IP address from that header only when the connection comes from one of these ranges. Rate limits and the audit log use the client IP address.

The default is `127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `::1/128` and `fc00::/7`. That fits the bundled Caddy on the Docker network. A list that you set replaces the default.

Turmite accepts ranges and single IP addresses, separated by spaces or commas. The bundled Caddy reads the same variable for its own `trusted_proxies`, and Caddy needs spaces. Use spaces:

```dotenv
TURMITE_TRUSTED_PROXIES=10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 173.245.48.0/20
```

See [Reverse proxy, Cloudflare and firewall](/docs/reverse-proxy/#client-ip-addresses).

### TURMITE_HOT_RECIPIENT

An age public key (`age1...`). When you generate or import a hot key in the dashboard, the `api` encrypts it to this key. The `api` cannot decrypt it. `turmite init` writes this value.

### TURMITE_HOT_IDENTITY_FILE

The path to the age identity that matches `TURMITE_HOT_RECIPIENT`. The file is encrypted with the hot-wallet passphrase. Only the `worker` reads it.

```dotenv
TURMITE_HOT_IDENTITY_FILE=/secrets/hot-identity.age
```

The compose file mounts `secrets/` read-only at `/secrets` in the `worker` container only.

### TURMITE_HOT_PASSPHRASE and TURMITE_HOT_PASSPHRASE_FILE

The passphrase that decrypts the identity. Set one of the two. Only the `worker` reads them.

- `TURMITE_HOT_PASSPHRASE_FILE` points to a file. The worker can then restart without you.
- `TURMITE_HOT_PASSPHRASE` holds the passphrase. Use it to type the passphrase at start. See [Run without a stored passphrase](/docs/install/#run-without-a-stored-passphrase).

When both are set, `TURMITE_HOT_PASSPHRASE` wins. When `TURMITE_HOT_IDENTITY_FILE` is set and neither is set, the worker refuses to start. A wrong passphrase stops the start with "wrong hot-wallet passphrase".

### TURMITE_SMTP_URL

The SMTP server, with credentials in the URL.

```dotenv
# TLS on port 465
TURMITE_SMTP_URL=smtps://user:password@smtp.example.com:465
# STARTTLS on port 587
TURMITE_SMTP_URL=smtp://user:password@smtp.example.com:587?tls=required
```

A `smtp://` URL without `?tls=required` sends mail without encryption. Percent-encode special characters in the user name and the password, for example `@` as `%40`.

Turmite sends two kinds of email: payout approval codes, and notices of critical or high security advisories to root and admin users. On testnet, SMTP is optional: without it, Turmite writes each email to the log. On mainnet, Turmite starts without SMTP, but each approval code fails with "Email is not set up. Set TURMITE_SMTP_URL.", so held payouts cannot be approved.

### TURMITE_SMTP_FROM

The sender address of the emails, with or without a name.

```dotenv
TURMITE_SMTP_FROM=Turmite <payments@example.com>
```

Use an address on a domain with SPF and DKIM for your SMTP provider. Otherwise the codes can land in spam.
