---
title: Troubleshooting
description: Find and fix common problems with the install, sign-in, payments, sweeps, payouts and webhooks.
---

Run each `docker compose` command as root in `/opt/turmite`.

## Read the logs first

```bash
sudo docker compose ps
sudo docker compose logs --tail 200 api
sudo docker compose logs --since 1h worker | grep -i -E 'warn|error'
sudo docker compose logs caddy
```

Each log line is a JSON object. Errors name the network, the payment or the delivery that failed. See [Monitoring and logs](/docs/monitoring/).

## Install and Docker

### Caddy cannot get a certificate

- The DNS record must point to this server. Check with `dig +short pay.example.com`.
- Ports 80 and 443 must be open to the internet. Check the provider firewall.
- Behind Cloudflare, see [Certificates behind Cloudflare](/docs/reverse-proxy/#certificates-behind-cloudflare).

### Containers cannot resolve DNS

This happens with Tailscale or a local DNS stub (`nameserver 100.100.100.100` or `127.0.0.53`). Give Docker other resolvers:

```bash
echo '{ "dns": ["1.1.1.1", "9.9.9.9"] }' | sudo tee /etc/docker/daemon.json
sudo systemctl restart docker
```

If `daemon.json` already exists, add the `dns` key to it. Do not overwrite the file.

### docker compose cannot read .env

Only root can read `/opt/turmite/.env`. Run `docker compose`, `backup.sh` and `restore.sh` with `sudo`. Do not make `.env` readable for other users. It holds `TURMITE_SECRET` and the database password.

### The install script stops

| Message | Fix |
|---|---|
| `/opt/turmite/.env exists. Use --upgrade, or remove /opt/turmite to start again.` | Upgrade with `--upgrade`, or remove the directory |
| `pass --site-url https://pay.example.com` | Give `--site-url`. Without a terminal, or with `--yes`, the script cannot ask |
| `set TURMITE_HOT_PASSPHRASE, run in a terminal, or pass --yes` | Run the script in a terminal, or pass `--yes` for a random stored passphrase |
| `this script supports Ubuntu and Debian; install Docker Engine and the Compose plugin yourself` | Install Docker, then run the script again |
| `the API did not become healthy; see the logs above` | Read the log lines above the message. See [The api exits at start](#the-api-exits-at-start) |

### The api exits at start

Read the last lines of its log: `sudo docker compose logs --tail 50 api`. A start-up error is one line that starts with `error:`.

| Message | Fix |
|---|---|
| `set TURMITE_SECRET (64 hex characters; ...)` | Set `TURMITE_SECRET`. `turmite init` writes one |
| `TURMITE_SECRET must be hex` or `must be 64 hex characters (32 bytes)` | Use 64 hex characters, for example from `openssl rand -hex 32` |
| `TURMITE_MODE must be mainnet or testnet` | Fix the value of `TURMITE_MODE` |
| `this database was set up for testnet; TURMITE_MODE is mainnet` | Set `TURMITE_MODE` back to the mode of the database, or use a new database for the other mode |
| `TURMITE_SITE_URL must be an http(s) URL` or `must not have a query or fragment` | Use a full `http://` or `https://` URL, without a query or a fragment |
| `mainnet needs an https:// TURMITE_SITE_URL` | Use an `https://` site URL on mainnet |
| `TURMITE_TRUSTED_PROXIES: bad range` | Use CIDR ranges or IP addresses, separated by spaces |
| `TURMITE_SMTP_URL: ...` or `TURMITE_SMTP_FROM: ...` | Fix the SMTP value. Percent-encode special characters in the URL |
| `database error: ...` | Check `TURMITE_DATABASE_URL` and that `postgres` runs |
| `internal error: migration failed: ...` | Restore the last dump before you try again, then report the error |

### The worker exits at start

| Message | Fix |
|---|---|
| `set TURMITE_HOT_PASSPHRASE or TURMITE_HOT_PASSPHRASE_FILE to unlock the hot-wallet identity` | Give the passphrase. See [Run without a stored passphrase](/docs/install/#run-without-a-stored-passphrase) |
| `wrong hot-wallet passphrase` | Give the passphrase that you used with `turmite init` |
| `read /secrets/hot-identity.age: ...` | The file is missing, or the worker user cannot read it. `secrets/` must belong to `65532:65532` |
| `the hot-wallet identity cannot read the deposit seed (wrong identity file?)` | The identity in `secrets/` does not belong to this database. Restore `secrets/` from the same install as the database |

To give `secrets/` back to the worker user:

```bash
sudo chown -R 65532:65532 /opt/turmite/secrets
```

## Sign-in

### "Too many attempts. Wait a minute."

Sign-in allows 10 attempts per minute per IP address and per email address. Wait one minute.

If every user sees this message, Turmite sees one IP address for all visitors. Check `TURMITE_TRUSTED_PROXIES` and the proxy headers. See [Client IP addresses](/docs/reverse-proxy/#client-ip-addresses).

### Two-factor codes are refused

1. Check the server clock: `timedatectl`. It must show `System clock synchronized: yes`.
2. Turn on time sync if it is off: `sudo timedatectl set-ntp true`.
3. Wait for the next code. Each code works one time.

After 5 wrong codes, Turmite refuses codes for that user for up to 15 minutes: "Too many wrong codes. Wait 15 minutes."

### Lost authenticator device

The root user can click **Reset 2FA** for the user under **Server → Users**. Without the root account, run:

```bash
sudo docker compose exec api turmite user reset2fa user@example.com
```

The user signs in with the password and enrols again.

### A form fails after sign-in

Turmite refuses a dashboard `POST` whose `Origin` (or `Referer`) header does not have the origin of the site URL. The answer is `403 Cross-site request refused.`

- Open the dashboard on the host of `TURMITE_SITE_URL`, or on a private entrance that rewrites `Origin`. See [Private dashboard](/docs/private-dashboard/).
- Check that your proxy does not remove or change `Origin`.
- Check that `TURMITE_SITE_URL` has the correct scheme, host and path.

### The session ends at once

On an `https://` site URL, the cookie has the `Secure` flag. A browser does not send it over `http://`. Open the dashboard over `https://`.

## Payments

### A deposit does not show

1. Open **Server → Networks**. The network must show **Connected**, and no **No new block** badge. If it does not, the nodes fail or rate limit you. Add a node.
2. Check that the token is on for that network.
3. Check that the customer used the correct network and the correct token contract.
4. Search the worker log for the address or the transaction hash.

### A payment stays in confirming

The deposit needs the confirmation count of the network. A Bitcoin block can take more than an hour. The checkout shows the count, for example `1 of 2`. The defaults are in [Networks and nodes](/docs/nodes/#confirmations).

### The customer sent too little

The payment shows `partially_paid`. The checkout shows the missing amount. The customer can send the rest to the same address. Turmite watches the address until one day after the payment window closes. A payment that is still short then becomes `expired`. See [Partial payments](/docs/statuses/#partial-payments).

### The customer paid after expiry

A deposit that arrives within one day after the payment window still counts. The payment continues to `finished`. The payment page in the dashboard shows the settlement **Paid late**. Check the price before you ship.

Turmite does not watch the address after that day. Check a later deposit on a block explorer and decide by hand: refund with a payout, or use **Mark as paid**.

### The checkout status does not change

The status frame reloads every 4 seconds. Check the worker log for the payment. A browser extension that blocks frames can stop the reload. A page reload shows the current status.

## Wallets

### The first Bitcoin addresses do not match

Turmite derives native SegWit (BIP84) receive addresses from the account key. Use the account key of a BIP84 account: `xpub` or `zpub` on mainnet, `tpub` or `vpub` on Signet. A key of another account type gives other addresses.

### Monero payments do not show

- A restore height above the wallet's creation height hides older payments.
- Check the private view key. It must belong to the primary address.
- Check that the Monero node is synced.

### An EVM factory deploy fails

The project's EVM hot wallet pays the gas for the deploy. Fund it on that network and try again.

## Sweeps

### Tron, Solana or TON funds stay at the deposit address

1. Check the hot wallet balance on that network in **Server → Hot wallets**.
2. Read the worker log for sweep errors.
3. Check that the worker runs. Without the passphrase, the worker does not start, and nothing is swept.

| Network | Common cause |
|---|---|
| Tron | Not enough TRX for energy and bandwidth on a USDT sweep |
| Solana | Not enough SOL for fees, or for rent of a new token account |
| TON | Not enough TON. The first sweep also deploys the deposit wallet |

### EVM funds stay in the deposit contract

The hot wallet needs gas for the flush. A token that is paused or that blocks the address can also fail. Funds in a deposit contract can only go to your cold wallet.

## Payouts

### A payout stays in pending

The worker signs payouts. Check that it runs and that it unlocked the hot identity:

```bash
sudo docker compose logs --since 1h worker | grep -i -E 'passphrase|identity|payout'
```

### No approval email

- Check `TURMITE_SMTP_URL` and `TURMITE_SMTP_FROM`.
- Read the api log for SMTP errors.
- Percent-encode special characters in the SMTP password.

### A payout is in needs-review

Turmite cannot tell if the transaction landed. Check the hot wallet's history on a block explorer, then resolve the payout in the dashboard. Do not create a new payout before you know. See [Payouts](/docs/payouts/).

## Webhooks

### Every delivery fails

- On mainnet, the URL must be public `https`. Turmite refuses private and reserved IP addresses.
- Turmite does not follow redirects. Use the final URL.
- Your endpoint must answer with a `2xx` status.
- Use **Send test** on the Webhooks page to try again.

### The signature does not match

- Use the correct secret. A project webhook endpoint signs with its own signing secret, which the Webhooks page shows one time, when you add the endpoint. The IPN of a payment (`ipn_callback_url`) signs with the IPN secret of the project.
- For `x-turmite-signature`, compute the HMAC over the raw body bytes. Some frameworks parse the body first and change it.
- For the HMAC-SHA512 signature, sort the keys of the JSON body before you sign. See [Webhooks](/docs/webhooks/).
- Read the name of the HMAC-SHA512 header in **Settings → General**. The default is `x-turmite-sig`.

### The timestamp is outside your window

Your server clock is wrong. Turn on time sync: `sudo timedatectl set-ntp true`.

## Pay with another coin

### The checkout does not show "Pay with another coin"

- The install runs on testnet. Trocador swaps mainnet coins only.
- The exchange is off, or the project has no Trocador API key. See [Pay with any coin](/docs/exchange/#turn-it-on).
- The payment is not `waiting` or `partially_paid`, or its window closed.

### Errors on the swap page

| Message | Cause | What to do |
|---|---|---|
| The amount to send, ..., is less than the exchange minimum | The amount is below the minimum for that coin | The customer chooses another coin or pays directly |
| The amount to send, ..., is more than the exchange maximum | The amount is above the maximum for that coin | Same as above |
| The exchange has no quote for this coin | No exchange with the KYC rating that you allow offers this pair now | Allow a lower KYC rating, or the customer chooses another coin |
| The exchange cannot do this swap now | Trocador refused the trade or did not answer | Try again later. The customer can pay directly |
| The exchange returned no deposit address | Trocador answered without an address | Try again |
| The exchange cannot send this payment's coin | Trocador does not list the payment's coin and network | The customer pays directly. Or accept a coin that Trocador lists |

### Saving the API key fails

"The exchange did not accept the key" or "The exchange refused the API key" means that Trocador refused it. Check the key in your Trocador account. Turmite does not save a refused key.

### The customer paid the exchange, but the payment is not paid

Turmite credits only what reaches the deposit address on chain. Open the payment in the dashboard and read its deposits. The customer's swap page shows the swap status and the swap ID. When the exchange sent less than the amount due, the payment is `partially_paid`. A refunded swap sends the coins to the refund address that the customer entered. Without a refund address, the customer contacts the exchange's support with the swap ID.

## Database

Open a PostgreSQL shell:

```bash
sudo docker compose exec postgres psql -U turmite turmite
```

Do not change rows by hand unless a release note or a maintainer asks you to. Take a dump first.

## Ask for help

Open an issue on [GitHub](https://github.com/loruorg/turmite/issues). Add the Turmite version, the mode, the network and the log lines. Remove addresses and order IDs that you do not want to publish. Report security problems privately. See [Reporting a vulnerability](/docs/report-vulnerability/).
