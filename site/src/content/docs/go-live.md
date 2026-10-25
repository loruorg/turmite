---
title: Go-live checklist
description: Check the server, the accounts, the wallets and one real payment per network before you take mainnet payments.
sidebarTitle: Go-live checklist
---

Complete each item before you send real customers to Turmite.

## Server

- [ ] The server runs only Turmite. Your shop runs on another machine.
- [ ] SSH accepts keys only. `PasswordAuthentication no` is set in `/etc/ssh/sshd_config`.
- [ ] Automatic security updates are on (`unattended-upgrades` on Debian and Ubuntu).
- [ ] The clock is synchronized. `timedatectl` shows `System clock synchronized: yes`.

## Network and TLS

- [ ] The provider firewall (or `DOCKER-USER`) allows only 22 from your addresses, and 80 and 443. See [Firewall](/docs/reverse-proxy/#firewall).
- [ ] PostgreSQL and port 8080 are not reachable from the internet.
- [ ] `https://pay.example.com/healthz` returns `ok` with a valid certificate.
- [ ] Behind Cloudflare: SSL/TLS is **Full (strict)**, and ports 80 and 443 accept only Cloudflare's ranges.
- [ ] The audit log shows real visitor IP addresses. See [Client IP addresses](/docs/reverse-proxy/#client-ip-addresses).

## Dashboard and accounts

- [ ] `/dashboard` is blocked or returns `404` on every public host name. See [Private dashboard](/docs/private-dashboard/).
- [ ] Every user has two-factor authentication. On mainnet, Turmite refuses sensitive actions until it is on.
- [ ] Each user has the lowest role that works. No unused admin accounts exist.
- [ ] The root password and the root TOTP seed are stored offline.

## Install

- [ ] `TURMITE_MODE=mainnet`.
- [ ] `TURMITE_SITE_URL` uses `https://`.
- [ ] SMTP works. A test payout approval email arrived.
- [ ] Update checks are on, or you watch the GitHub releases and advisories.
- [ ] `TURMITE_VERSION` in `.env` names the release that you tested.

## Wallets and keys

- [ ] Cold wallets are hardware wallets or multisigs.
- [ ] Bitcoin: the first receive addresses on the **Wallets** page match the first receive addresses in your wallet.
- [ ] Monero: the primary address, view key and restore height are correct.
- [ ] EVM: a factory is deployed on each EVM network you accept. A multisig cold wallet exists on each of these networks.
- [ ] Tron, Solana, TON: the cold wallet addresses are correct.
- [ ] Each hot wallet holds about one week of fees and payouts, and no more.

## Nodes

- [ ] Each enabled network has at least two nodes that you run or pay for.
- [ ] Each network shows **Connected** and a recent block on **Server → Networks**.
- [ ] Confirmation counts are at the defaults or higher.

## Payouts

- [ ] **Approve automatically up to**, **Hourly limit** and **Daily limit** are set low, in USD.
- [ ] The address book contains only addresses that you checked.
- [ ] Only trusted people have the **Project admin** role.

## Integration

- [ ] The shop keeps the API key on the server. The key is not in any browser code.
- [ ] The webhook handler checks the signature. See [Which signature to use](/docs/webhooks/#which-signature-to-use).
- [ ] The handler ships only on `finished`, and it is safe to call twice.
- [ ] **Send test** on the Webhooks page reaches the shop.
- [ ] The shop keeps the IPN secret and the webhook signing secrets on the server, and reads the signature header that **Settings → General** shows.

## Backups

- [ ] `backup.sh` runs each night from root's crontab, and the dumps go off the server.
- [ ] `.env`, `secrets/` and the hot wallet passphrase are stored offline.
- [ ] You restored a backup on a spare server once.

## Monitoring

- [ ] An uptime monitor checks `/healthz`.
- [ ] You get an alert for `ERROR` log lines.
- [ ] Someone watches hot wallet balances, dead webhook deliveries and payouts in `needs-review`.

## Move from testnet to mainnet

The mode is fixed for the life of an install. To go live after tests on testnet:

1. Make a new install with `--mode mainnet` and a new database. See [Install on a server](/docs/install/).
2. Enter mainnet wallets: a mainnet xpub (`xpub` or `zpub`), a mainnet Monero view key and mainnet cold wallet addresses.
3. Add mainnet nodes.
4. Create new API keys, copy the new IPN secret and add the webhook endpoints again.
5. In your shop, set the new base URL, API key and IPN secret.

Keep the testnet install for tests of new versions.

## Final test

- [ ] One small real payment on each network went from checkout to `finished`.
- [ ] The funds arrived in the cold wallet or your own wallet.
- [ ] The shop received the webhook and marked the order paid.
- [ ] One small payout went through approval and reached its address.
