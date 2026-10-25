---
title: Dashboard and roles
description: Sign in to the dashboard, give each user the lowest role that works, and protect accounts with two-factor codes.
sidebarTitle: Dashboard and roles
---

The dashboard is at `/dashboard` under your site URL. The server renders every page, and every action is an HTML form. It works with JavaScript off.

## First sign-in

1. Open `https://pay.example.com/dashboard/setup`.
2. Enter an email address and a password of 12 characters or more. Enter the password again.
3. Click **Create account**. This account becomes the `root` user. There is only one `root` user. If the install has no project, Turmite also makes a project called "My shop".
4. Open **Account** and turn on two-factor authentication.

After sign-in, Turmite opens your first project. An admin with no project goes to **New project**. A member with no project goes to **Account**.

The setup page works only while the database has no users. After that, sign in at `/dashboard/login`.

## Pages

The **Project** menu at the top of the sidebar switches between projects. Admins see **New project** there. Each project has these pages, in this order:

| Page | Use |
|---|---|
| Overview | Setup steps, totals of the last 30 days and recent payments. A warning lists payments whose sweep is late or failed |
| Payments | List and detail of payments. The detail page shows each sweep with its status, and has **Mark as paid** and **Mark as refunded** |
| Invoices | Invoices, and **New payment link** to make one by hand |
| Wallets | Settlement setup per network, and **Deploy now** for the EVM factory. See [Wallets per chain](/docs/wallets/) |
| Payouts | Tabs **Payouts**, **New payout**, **Address book** and **Limits**. See [Payouts](/docs/payouts/) |
| API keys | Create and revoke keys for the merchant API |
| Webhooks | Endpoints with **Send test** and **Delete**, and deliveries with **Retry**. See [Webhooks](/docs/webhooks/) |
| Settings | Tabs **General** (name, payment and invoice windows, IPN signature header, IPN secret), **Checkout** (logo, accent colour, theme, message, support contacts, return URL and hosts), **Exchange** (Trocador) and **Domain** (checkout domain) |
| Team | Project members and their roles |

Global admins also see a **Server** section in the sidebar:

| Page | Use |
|---|---|
| Server → Networks | Per network: on or off, confirmations, nodes with status, height and the node in use, last block, assets and tokens |
| Server → Hot wallets | Make or import hot wallets, see their balances, assign them to projects |
| Server → Users | Add users, set the global role, disable users and reset two-factor |
| Server → Audit log | Every sign-in and sensitive action, 100 entries per page |
| Server → Updates | The installed version, the latest release, security advisories, **Check now** and the daily check setting |

## Roles

Each user has one global role. Members also have one role in each project they belong to.

### Global roles

| Role | Access |
|---|---|
| `root` | Everything. The first user. Only `root` adds admins, changes global roles, disables admins and resets two-factor for other users |
| `admin` | Every project with the `project_admin` role, plus new projects, checkout domains, networks, nodes, hot wallets, users, the audit log and updates |
| `member` | Only the projects the user belongs to, with that project's role |

### Project roles

| Role | Access |
|---|---|
| `project_admin` | Everything in the project except the checkout domain: settings, IPN secret rotation, wallets, creating API keys, team, **Mark as paid** and **Mark as refunded**, payouts, approvals, the address book and payout limits |
| `developer` | Create payment links, add and delete webhook endpoints, send tests, retry deliveries, revoke API keys, show the IPN secret. Read everything else |
| `read_only` | Read access to the project |

Give each person the lowest role that works. Use `developer` for integration work and `read_only` for support staff. Give `project_admin` only to people who can approve payouts.

## Two-factor authentication

Turmite uses TOTP codes from an authenticator app.

1. Open **Account**. The link is your email address at the foot of the sidebar.
2. Under **Two-factor authentication**, click **Set up two-factor**.
3. Scan the QR code with an authenticator app, or enter the key that the page shows.
4. Enter the code that the app shows, and click **Turn on**.

To turn two-factor off, enter a current code and click **Turn off**. On mainnet, Turmite refuses sensitive actions while two-factor is off. On testnet, those actions work without a code until you turn two-factor on.

After you turn on two-factor authentication, Turmite asks for a code at each sign-in.

### Actions that need a fresh code

A sensitive action needs a new code, also during an active session:

- creating API keys;
- showing or rotating the IPN secret;
- saving a wallet, and **Deploy now**;
- saving the exchange settings, and removing the Trocador key;
- setting the checkout domain;
- creating, approving, rejecting, cancelling and resolving payouts;
- adding and removing address book entries;
- saving payout limits;
- **Mark as paid** and **Mark as refunded** on a payment;
- making, importing and assigning hot wallets;
- adding or removing an RPC node, lowering the confirmations of a network, turning assets on or off, adding a token;
- adding a user, making a user an admin, and resetting two-factor for a user;
- adding a project admin to a team;
- turning off update checks;
- turning two-factor off for your own account.

These actions need no code: revoking an API key, raising confirmations, turning a network on or off, stopping or starting payments on a network, the **General** and **Checkout** settings, other team changes, making an admin a member, disabling users, webhook endpoints and payment links. A new password needs the current password.

Each code works one time. After 5 wrong codes, Turmite locks two-factor checks for that user for up to 15 minutes. The lock is in memory. It ends early when the `api` process restarts.

### Lost authenticator

The `root` user can reset two-factor for another user under **Server → Users**, with a fresh code. Without the root account, a person with shell access to the server can clear two-factor for a user:

```bash
cd /opt/turmite
sudo docker compose exec api turmite user reset2fa user@example.com
```

The command also signs the user out of every session. The user then signs in with the password and enrols again. The audit log records the reset as `user.2fa_reset`.

## Sign-in protection

- Passwords have at least 12 characters.
- Turmite hashes passwords with Argon2id (64 MiB, 3 passes). At most four hashes run at one time.
- Sign-in is limited to 10 attempts per minute per IP address and per email address.
- Sign-in with two-factor has two steps: the password at `/dashboard/login`, then the code at `/dashboard/login/code`. The second step must follow within 5 minutes.
- A disabled user cannot sign in, and the user's sessions end.
- A password change ends the other sessions of the user. The session that changed the password stays.
- Turmite takes the client IP from `X-Forwarded-For` only when the request comes from `TURMITE_TRUSTED_PROXIES`. See [Reverse proxy, Cloudflare and firewall](/docs/reverse-proxy/#client-ip-addresses).

## Sessions

| Property | Value |
|---|---|
| Cookie name | `turmite_session` |
| Flags | `HttpOnly`, `SameSite=Strict`, `Secure` on `https://` |
| Path | The path of the site URL, for example `/` or `/pay` |
| Lifetime | 12 hours without activity. Each request extends it, up to 7 days after sign-in |

Each dashboard `POST` request must carry an `Origin` header, or a `Referer` header, with the origin of the site URL. Turmite refuses other requests with `403 Cross-site request refused.` A proxy that changes `Origin` breaks the dashboard. See [Private dashboard](/docs/private-dashboard/) for a private entrance that keeps this check.

## Audit log

Turmite writes an audit entry for each sign-in and each sensitive action, for example two-factor changes, API keys, the IPN secret, project settings, wallets, hot wallets, payouts, networks, nodes, manual payment changes and user changes. The worker adds `node.failover` when it moves a network to another node. Each entry has the time, the user or `system`, the project, the action, the target and the IP address. Global admins read it in **Server → Audit log**. Project admins do not see the audit log.

## Update checks

A minute after start and then once a day, the worker reads the latest release and the security advisories of `loruorg/turmite` on GitHub. The request sends the version in its `User-Agent` and nothing else. When a newer release exists, admins see a banner with **Dismiss**. When an advisory affects the installed version, admins see a red banner that stays until the server runs a fixed version. When SMTP is set, admins also get one email for each critical or high advisory.

The check is on by default. To turn it off, open **Server → Updates**, clear **Check for updates every day**, enter a two-factor code and click **Save**. If you turn it off, watch the [releases](https://github.com/loruorg/turmite/releases) and [advisories](https://github.com/loruorg/turmite/security/advisories) yourself.

To install an update, see [Upgrades](/docs/upgrades/).
