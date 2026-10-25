---
title: Threat model
description: The assets Turmite protects, its trust boundaries, each threat with its mitigation, and the risks that remain.
---

This page lists what Turmite protects, where the trust boundaries are, and how each threat is handled. The last section lists the risks that remain. The [Security model](/docs/security-model/) gives the short version.

## Assets

| Asset | Where it is | Impact if an attacker gets it |
|---|---|---|
| Customer deposits on EVM networks | CREATE2 deposit contracts | Delayed sweeps. A deposit contract can only flush to the cold wallet of its factory, so the attacker cannot take the funds |
| Customer deposits on Tron, Solana, TON | Per-payment keys from the deposit seed | Loss of deposits that are not yet swept |
| Bitcoin and Monero receipts | Your own wallet | None. The server holds no spend key |
| Cold wallets | Your hardware wallet or multisig | Total loss. Turmite never holds these keys |
| Hot wallets | PostgreSQL (`hot_wallets`), age-encrypted. Only the worker can decrypt | Loss of the hot balance |
| Deposit seed | PostgreSQL (`settings`), age-encrypted to `TURMITE_HOT_RECIPIENT`. Only the worker can decrypt | Loss of unswept deposits on Tron, Solana, TON |
| `TURMITE_SECRET` | `/opt/turmite/.env`, in the `api` and `worker` containers | Forged payout API tokens. With a copy of the database: TOTP secrets, IPN and webhook secrets, node URLs, view keys and Trocador keys |
| Hot identity and passphrase | `/opt/turmite/secrets/`, in the `worker` container only | With a copy of the database: hot keys and the deposit seed |
| Dashboard accounts | Argon2id hashes and sealed TOTP secrets in PostgreSQL | Payouts, wallet changes and configuration changes, as the role allows |
| API keys | SHA-256 hashes in PostgreSQL | Payment and invoice creation. API payouts also need a project admin's password and two-factor code |
| Payment state and webhooks | PostgreSQL and your endpoint | Your shop ships an order that is not paid |

## Trust boundaries

1. Internet to Caddy to `api`. The `api` role holds no private key. It can store payout requests. It cannot sign them.
2. `api` to PostgreSQL to `worker`. The two roles share only the database. A compromised `api` can write rows. The worker signs what the database holds.
3. Turmite to RPC nodes. Nodes are not trusted with keys. They are trusted to report deposits.
4. Turmite to webhook endpoints and logo URLs. On mainnet, outbound requests go only to public `https` addresses.
5. Turmite to your cold wallet. Turmite knows only addresses and public keys. On EVM networks the factory address is computed from the cold wallet (CREATE2). The same cold wallet always gives the same factory, and no other code can be at that address. The worker deploys the factory from the project's EVM hot wallet when you click **Deploy now**, or before the first sweep. You can also send the deploy transaction from your own wallet.

## Threats and mitigations

| # | Threat | Mitigation |
|---|---|---|
| T1 | Code execution in the `api` process | The `api` container holds no private key, no deposit seed and no hot passphrase. It holds `TURMITE_SECRET` and the database password, so the attacker can do what T4 describes and read the sealed secrets. Residual: R1, R2 |
| T2 | Code execution in the `worker` process | The worker opens no port, which makes it harder to reach. If it is reached, the attacker gets the hot wallets and unswept deposits on Tron, Solana and TON. Residual: R2, R3 |
| T3 | Read access to the database | Passwords use Argon2id. TOTP secrets, IPN and webhook secrets, node URLs, view keys and Trocador keys are sealed with `TURMITE_SECRET`, which is not in the database. Hot keys and the deposit seed are age-encrypted to an identity that a passphrase protects. Neither the identity nor the passphrase is in the database. API keys and session tokens are SHA-256 hashes. Payout approval codes are keyed hashes |
| T4 | Write access to the database | The attacker can change payment state and set payouts to `pending`, which the worker signs. The attacker can change a project's wallets, so that new payments and unswept Tron, Solana and TON deposits go to the attacker. The attacker cannot change where existing EVM deposits go. Residual: R1, R2 |
| T5 | Stolen dashboard password | Login needs a TOTP code when two-factor is on. Sensitive actions need a fresh code. On mainnet they are refused without two-factor. After 5 wrong codes, checks lock for up to 15 minutes. Login is limited to 10 attempts a minute per IP and per email |
| T6 | Stolen API key | The key can create payments and invoices in its own project and read them. API payouts also need a bearer token from `POST /v1/auth` with a project admin's email and password, then that admin's two-factor code. API payouts go only to address-book entries. Payouts above the limits wait for approval |
| T7 | Malicious or faulty RPC node | Several nodes per network. The worker probes them every 30 seconds and moves to the next node after two failed probes, or when a node falls too far behind. EVM nodes on another chain ID, Solana nodes with another genesis hash, and Tron and TON nodes on a network of the wrong mode are refused. Node URLs are sealed. The dashboard and the audit log show them without credentials. Residual: R4 |
| T8 | Chain reorganisation | A deposit counts only after the network's required confirmations. A Bitcoin deposit that leaves the mempool without a block stops counting |
| T9 | Forged or replayed webhook | Every delivery has an HMAC-SHA512 signature over the key-sorted body and an HMAC-SHA256 signature over the timestamp and the raw body. Receivers can refuse old timestamps |
| T10 | Server-side request forgery through webhook or logo URLs | On mainnet only public `https` URLs. Private and reserved IP addresses are refused at connect time, after DNS resolution. Redirects are not followed |
| T11 | Cross-site scripting or framing of the checkout | No JavaScript on the checkout. A strict content security policy with `default-src 'none'` and `frame-ancestors 'none'`. Server-side templates escape output |
| T12 | Cross-site request forgery on the dashboard | `SameSite=Strict` session cookie. Every dashboard `POST` must carry an `Origin` or `Referer` with the origin of the site URL |
| T13 | Brute force and resource exhaustion | Login limit of 10 a minute per IP and per email. TOTP lockout. At most four password hashes at one time. 600 merchant API requests and 300 checkout requests a minute per IP. `X-Forwarded-For` is trusted only from `TURMITE_TRUSTED_PROXIES` |
| T14 | Testnet install pointed at mainnet nodes, or the reverse | `TURMITE_MODE` seeds the networks and token contracts of one mode. EVM, Solana, Tron and TON nodes of the wrong chain are refused. A testnet install accepts a local Tron or TON chain. A Bitcoin account key must match the mode and the node's network. A mainnet install refuses Monero test-network addresses |
| T15 | Double payout | Each withdrawal can carry a `unique_external_id`. The worker signs and stores a transaction before it broadcasts it. When the outcome is unknown, the payout becomes `needs-review`. EVM payouts and sweeps share a nonce lock per hot wallet |
| T16 | Phishing through a checkout domain | Only a server admin with a fresh code sets a checkout domain. On that host Turmite serves only the checkout, the merchant API, assets and `/healthz`. The dashboard is not served there |
| T17 | Hostile customer input on the checkout | Checkout forms take only the coin choice and, for a swap, a coin search and a refund address with an optional memo. Forms post to the same origin only (`form-action 'self'`) |
| T18 | Trocador or an exchange reports a false status, sends less than the amount due or sends nothing | Turmite does not trust the exchange. The exchange delivers to the payment's own deposit address, and the normal watcher credits only what arrives on chain with the required confirmations. The swap status is for display only. A call from Trocador only makes Turmite fetch the swap again with the project's key. The Trocador API key is sealed with `TURMITE_SECRET`. Residual: R6 |
| T19 | Stolen backups | Database dumps contain sealed secrets, encrypted hot keys and the encrypted deposit seed. They are useless without `TURMITE_SECRET` and the hot identity with its passphrase. Store `.env` and `secrets/` apart from the dumps |
| T20 | Vulnerable release in use | The worker checks GitHub releases and security advisories once a day. Admins see a banner, and an email for critical and high advisories when SMTP is set |

## Residual risks

- R1. Database write access can approve payouts. Approval state lives in PostgreSQL, and the worker trusts it. Code execution in the `api` process gives this access. Keep the hot balance small and keep PostgreSQL off the network. The Docker Compose stack does not publish its port.
- R2. Tron, Solana and TON deposits are custodial until the sweep. Anyone with the deposit seed can move them. Anyone who can write the database can change where the worker sweeps them. Keep sweeps running, and use the EVM networks, Bitcoin or Monero for large amounts.
- R3. Hot wallets can be spent by anyone who controls the worker. Keep about a week of fees and planned payouts in them, no more.
- R4. Turmite trusts the node that answers to report a deposit. It does not need several nodes to agree. Use your own nodes or paid providers, and check large orders on a block explorer before you ship.
- R5. An admin can approve a payout that the same admin created. Turmite does not require a second person. Teams should agree that a second person approves.
- R6. During a swap through Trocador, the exchange holds the customer's coins. If the swap fails or halts, the customer loses time or money until the exchange refunds. An exchange with KYC rating C or D can hold the coins for an identity check. Turmite never credits a payment without an on-chain deposit. See [Pay with any coin](/docs/exchange/).
- R7. The TOTP lockout and the rate limits are held in the memory of each process. A restart clears them, and with more than one `api` process each process counts on its own. Each code still works only one time, because the last used step is in the database.
- R8. Bitcoin payments depend on a correct account key. Compare the first receive addresses that the Wallets page shows with your own wallet. See [Wallets per chain](/docs/wallets/).
- R9. `turmite init` stores the hot passphrase in `secrets/hot_passphrase`, so the worker restarts unattended. A copy of `secrets/` then holds both the identity and its passphrase. If your risk model needs it, run `turmite init --no-store-passphrase` and set `TURMITE_HOT_PASSPHRASE` at each start.

## Out of scope

- The security of your cold wallet and its seed.
- The security of your shop and of the server that receives webhooks.
- Attacks on the blockchains themselves.
- A malicious operator. The operator controls the server and can change the software.
