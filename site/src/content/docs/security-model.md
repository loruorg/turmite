---
title: Security model
description: What a server compromise can and cannot do on each chain, and how Turmite limits the damage.
---

Turmite keeps the keys that receive money away from the server where the chain allows it. This page tells you what an attacker can do with full control of the server, per chain, and which controls limit the damage.

## What a server compromise can do

The table assumes the worst case: an attacker controls the server, the database and every secret on it.

On every chain, the attacker can change payment records, mark an unpaid order as paid, and change a project's wallet so that new payments go to the attacker.

| Network | Deposit address | The attacker can also | The attacker cannot |
|---|---|---|---|
| Bitcoin | Receive address of your BIP84 account key, in a watch-only wallet `turmite-p<project_id>` on your bitcoind | Nothing more | Spend any bitcoin. The server holds no Bitcoin private key |
| Monero | Subaddress of a view-only wallet made from your address and private view key | See incoming payments | Spend any monero. The view key cannot sign |
| Ethereum, Base, Polygon, BNB Smart Chain | CREATE2 deposit contract per payment | Spend the hot wallet balance | Send a deposit to another address. Each deposit contract can only flush to the cold wallet fixed in its factory |
| Tron, Solana, TON | Key per payment, derived from the install's deposit seed | Take deposits that are not yet swept. Spend the hot wallet balance | Take funds that are already in your cold wallet |

On Tron, Solana and TON, Turmite holds the deposit keys until the sweep. During that time the funds are custodial. The worker reads the sweep destination, the project's cold wallet, from the database at sweep time. Thus a person who can write to the database, for example through code execution in the `api` process, can change the cold wallet and send the deposits that are not swept yet to another address. The audit log does not record a direct database write. On the EVM networks this is not possible: each deposit contract flushes only to the cold wallet of its factory. To keep the exposure small:

- Keep the hot wallet funded, so that sweeps do not stop.
- Keep the hot wallet balance small. It only needs fees for sweeps and the payouts you plan.
- Prefer the EVM networks, Bitcoin or Monero for large amounts.

The Monero wallet files on the wallet RPC host have no password. A person who can read the wallet directory of that host gets the private view key of each project and sees its incoming payments. The person cannot spend. Keep the wallet RPC host private, and give it to Turmite only.

A compromised server can lie about payment state. For a large order, check the deposit on a block explorer before you ship.

## Process split

Turmite is one binary with two roles. Each role holds different secrets.

| Role | Faces the internet | Holds | Does not hold |
|---|---|---|---|
| `api` | Yes, behind Caddy | `TURMITE_SECRET`, the database password, `TURMITE_HOT_RECIPIENT` (an age public key) | Any private key, the deposit seed, the hot identity, the hot passphrase |
| `worker` | No. It opens no port | `TURMITE_SECRET`, the database password, the hot identity file and its passphrase | Nothing more is needed |

The `api` role derives a deposit address only where it needs no private key. On Tron, Solana and TON it takes an address from a pool that the worker fills. The `worker` role is the only process that can decrypt hot keys and the deposit seed. The two roles talk only through PostgreSQL.

Code execution in the `api` process gives the attacker no private key. It gives the attacker write access to the database. See [Threat model](/docs/threat-model/) T1 and R1.

The default Docker Compose stack in `/opt/turmite` runs the roles in separate containers:

- Only the `worker` container mounts `secrets/`, read-only.
- The `api` and `worker` containers have a read-only file system, drop all Linux capabilities and set `no-new-privileges`.
- The image is distroless. It has no shell and no package manager, and it runs as user 65532.
- PostgreSQL is on an internal network with no published port.

## Secrets

| Secret | Where | Protects |
|---|---|---|
| `TURMITE_SECRET` | `/opt/turmite/.env` | Seals TOTP secrets, IPN secrets, webhook endpoint secrets, RPC node URLs, Monero view keys and Trocador API keys in the database. Signs payout API tokens. Keys the hashes of payout approval codes |
| Hot identity (age) | `/opt/turmite/secrets/hot-identity.age`, encrypted with the passphrase | Decrypts hot wallet keys and the deposit seed |
| Hot passphrase | `/opt/turmite/secrets/hot_passphrase` (`TURMITE_HOT_PASSPHRASE_FILE`), or `TURMITE_HOT_PASSPHRASE` at start | Unlocks the hot identity |
| Database password | `/opt/turmite/.env` | Read and write access to every row |
| Cold wallet keys | Your hardware wallet or multisig | Everything that is swept or paid to you. Turmite never sees them |

`turmite init` stores the passphrase in `secrets/hot_passphrase`, so the worker can restart unattended. To keep it off the disk, run `turmite init --no-store-passphrase` and set `TURMITE_HOT_PASSPHRASE` each time the worker starts.

The `api` role encrypts every generated or imported hot key to `TURMITE_HOT_RECIPIENT`. It cannot decrypt what it encrypts.

A copy of the database alone does not reveal TOTP secrets, IPN and webhook secrets, node URLs, view keys, Trocador keys, hot keys or the deposit seed. Passwords are Argon2id hashes. API keys and session tokens are SHA-256 hashes. Store `.env` and `secrets/` apart from the database backups. See [Backups and restore](/docs/backups/).

## Accounts and sessions

- Passwords have at least 12 characters. They are hashed with Argon2id (64 MiB, 3 passes, 1 lane). At most four hashes run at one time, so a flood of logins cannot use all the memory.
- Login is limited to 10 attempts a minute per IP address and per email. `POST /v1/auth` shares this limit.
- Two-factor authentication (TOTP) is enrolled on the account page. When it is on, login needs a code.
- Each code works one time. The last used time step is stored in the database. After 5 wrong codes, two-factor checks for that user are locked for up to 15 minutes. The lock is in the memory of the `api` process and ends when the process restarts.
- Sensitive actions need a fresh code, for example API keys, the IPN secret, wallets, hot wallets, nodes, payouts and new users. See [Dashboard and roles](/docs/dashboard/#actions-that-need-a-fresh-code). On mainnet, these actions are refused until two-factor is on.
- The session cookie is `turmite_session`. It is `HttpOnly` and `SameSite=Strict`. It is `Secure` when the site URL uses `https`. Its path is the path of the site URL. A session ends after 12 hours without activity, and at the latest 7 days after sign-in.
- A disabled user cannot sign in. Turmite ends the user's sessions.
- A password change ends the other sessions of the user.
- Each process counts the login and API rate limits and the two-factor lockout in its own memory. With more than one `api` process, an attacker gets the limits one time for each process. A restart clears the counts.
- Every dashboard `POST` request must carry an `Origin` or `Referer` header with the origin of the site URL. Turmite refuses other requests.
- The audit log records each sign-in and each sensitive action with the time, the user or `system`, the project, the action, the target and the IP address.

The `root` user can reset two-factor for another user in the dashboard, with a fresh code. On the server, `turmite user reset2fa <email>` does the same. Both end the user's sessions and write an audit entry. See [CLI](/docs/cli/).

Roles limit what each user can do. See [Dashboard and roles](/docs/dashboard/).

## Web security

The checkout pages send this content security policy:

```text
default-src 'none'; style-src 'self'; style-src-attr 'unsafe-inline'; img-src 'self' data:; font-src 'self'; frame-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'
```

The checkout runs no JavaScript. Other sites cannot frame it. Only the live status frame (`/pay/{token}/status`) can be framed, and only by Turmite's own pages. The dashboard has its own policy with `script-src 'self'` and `frame-ancestors 'none'`. It works with JavaScript off. Its only script is optional: it copies text to the clipboard and asks for confirmation before some actions.

Turmite adds `X-Content-Type-Options: nosniff`, `Referrer-Policy: same-origin` and `X-Frame-Options` to its responses. Dashboard and checkout pages have `Cache-Control: no-store`. On an `https://` site URL, responses have `Strict-Transport-Security: max-age=31536000`.

Turmite pages load nothing from third parties. The fonts are served by Turmite. The QR code is an inline SVG that the server makes.

A project logo is an upload or an `https` URL that the server downloads one time. Turmite refuses SVG and files over 2 MB, decodes the image, scales it to fit 512 by 512 pixels and stores it as PNG. The checkout loads the stored copy from Turmite.

## Webhooks

- Every delivery is signed. The `ipn_callback_url` of a payment uses the project's IPN secret. Each project endpoint has its own signing secret. See [Webhooks](/docs/webhooks/).
- The `x-turmite-signature` header covers the `x-turmite-timestamp` value and the body, so your receiver can refuse replays.
- On mainnet, endpoints and logo URLs must be public `https` URLs. Turmite refuses private and reserved IP addresses when it connects. A DNS name that resolves only to such addresses is refused too.
- Turmite does not follow redirects.

## Payouts

Payouts are the only way Turmite sends money to an address that is not your cold wallet. Bitcoin and Monero have no payouts. These controls apply:

- Payouts come only from hot wallets. Keep their balance small.
- Limits per project, in USD: a minimum, an auto-approve amount, an hourly limit and a daily limit. By default the auto-approve amount is 0, so every payout waits for approval.
- A payout above the auto-approve amount, over the hourly or daily limit, or without a USD rate waits for approval.
- Approval needs a project admin's two-factor code. Turmite then emails that admin a 6-digit code. The code works for 10 minutes, 5 attempts and only for that admin. On mainnet, approvals need SMTP.
- Turmite does not stop an admin from approving a payout that the same admin created.
- A payout made in the dashboard needs a project admin and a fresh code. It can go to any valid address.
- An API payout needs the project API key and a bearer token from `POST /v1/auth` for a project admin. The batch waits as `pending-otp-verification` until that admin sends a two-factor code to `POST /v1/payout/{batch_id}/verify`. API payouts can only go to addresses in the address book.
- Changes to the address book and the limits need a fresh code.
- Each withdrawal can carry a `unique_external_id`. A retry with the same fields returns the existing payout. The same ID with other fields is refused.
- The worker signs and stores each transaction before it broadcasts it. When the outcome is unknown, the payout becomes `needs-review` and waits for an admin.
- EVM payouts and EVM sweeps from the same hot wallet share one nonce lock.

See [Payouts](/docs/payouts/).

## Nodes

Turmite trusts its RPC nodes to report deposits correctly. A malicious node cannot move funds, but it can report a deposit that did not occur. Turmite reads each network through one node at a time and does not compare nodes. An EVM node on another chain ID, a Solana node with another genesis hash, and a Tron or TON node on a network of the wrong mode are refused. Use your own nodes or paid providers, and add more than one node per network. See [Networks and nodes](/docs/nodes/).

## Updates and advisories

A minute after start and then once a day, the worker reads the latest release and the security advisories of `loruorg/turmite` on GitHub. The requests send the version in the `User-Agent` header and nothing else. Admins see a banner for a new release, and a red banner for an advisory that affects the running version. When SMTP is set, admins get one email for each critical or high advisory. The check is on by default. To turn it off, open **Server → Updates**. This needs a fresh code. See [Upgrades](/docs/upgrades/).

GitHub Actions builds each release image from a version tag, with an SBOM and a build provenance attestation. To check an image:

```bash
gh attestation verify oci://ghcr.io/loruorg/turmite:<version> --owner loruorg
```

## Operator duties

The software cannot do everything. Before you go live:

1. Turn on two-factor authentication for every user.
2. Keep the dashboard private. See [Private dashboard](/docs/private-dashboard/).
3. Use a hardware wallet or a multisig as the cold wallet.
4. Check the first deposit address against your own wallet. See [Wallets per chain](/docs/wallets/).
5. Keep hot wallet balances small and set low payout limits.
6. Back up the database, `.env` and `secrets/`, and store the copies in different places.
7. Use your own or paid RPC nodes, at least two per network.

The full list is in the [Go-live checklist](/docs/go-live/). The threats and the remaining risks are in the [Threat model](/docs/threat-model/).
