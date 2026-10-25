---
title: Projects and checkout domains
description: Run several shops on one install, and give each shop its own checkout domain.
sidebarTitle: Projects and domains
---

One install serves one operator. The operator can have many projects. A project is one shop or one business.

## What belongs to a project

| Per project | Shared by the install |
|---|---|
| Wallets per network | Users and global roles |
| API keys | Networks, RPC nodes, confirmations, tokens |
| Webhook endpoints and the IPN secret | Hot wallets (each project has its own assignment) |
| Branding and checkout settings | The deposit seed |
| Exchange settings | The exchange coin list |
| Checkout domain | Update checks |
| Payout limits and address book | Audit log |
| Team members and project roles | |

An API key works only for its own project. A payment always belongs to the project of the key that created it.

## Add a project

You need the `root` or `admin` role.

1. Open the project menu at the top of the sidebar.
2. Select **New project**.
3. Enter the **Name** of the shop, 1 to 80 characters.
4. Click **Create project**. The dashboard opens the **Wallets** page of the new project.

Then set up the project:

1. Open **Wallets** and add a wallet for each network you accept. See [Wallets per chain](/docs/wallets/).
2. Open **Settings → Checkout** and set the branding and the return URL. See [Checkout](/docs/checkout/#branding).
3. Open **API keys** and create a key for the shop.
4. Open **Webhooks** and add the shop's endpoint.
5. Open **Team** and add the people who work on this shop.

## Project settings

**Settings** has four tabs. Global admins have the `project_admin` role in every project.

| Tab | Settings | Who can save | Two-factor code |
|---|---|---|---|
| **General** | **Project name**, **Payment window (minutes)**, **Invoice window (minutes)**, **IPN signature header** | `project_admin` | No |
| **General** | IPN secret: **Show secret** | `developer` | Yes |
| **General** | IPN secret: **Rotate** | `project_admin` | Yes |
| **Checkout** | Logo, theme, accent colour, message, support links, **Return URL**, **Allowed return hosts** | `project_admin` | No |
| **Exchange** | Trocador API key, markup, KYC rating, allowed coins | `project_admin` | Yes |
| **Domain** | **Host name** of the checkout domain | `root` or `admin` | Yes |

General settings:

| Setting | Effect |
|---|---|
| **Project name** | 1 to 80 characters. The store name on the checkout pages |
| **Payment window (minutes)** | Time to pay once a coin is chosen. Default 60, from 5 to 10080. Then an unpaid payment becomes `expired` |
| **Invoice window (minutes)** | How long an invoice link stays open. Default 1440, from 5 to 43200 |
| **IPN signature header** | Name of the HMAC-SHA512 signature header. Default `x-turmite-sig`. Up to 64 letters, digits and hyphens. Other names that start with `x-turmite-` are refused. See [Webhooks](/docs/webhooks/) |

The Checkout tab is described in [Checkout](/docs/checkout/#branding). The Exchange tab is described in [Pay with any coin](/docs/exchange/#turn-it-on).

## How the addresses fit together

| Address | Example | Serves |
|---|---|---|
| Site URL (`TURMITE_SITE_URL`) | `pay.example.com` | Checkout, merchant API and the dashboard |
| Checkout domain (one per project, optional) | `pay.shop-two.com` | `/pay/`, `/i/`, `/x/`, `/v1/`, `/assets/`, `/branding/`, `/healthz` and `/favicon.svg` only |
| Private entrance (optional) | `turmite.example-tailnet.ts.net` | The dashboard, for your own devices. See [Private dashboard](/docs/private-dashboard/) |

On a checkout domain, `/dashboard` and every other path answer `404`. The checkout pages of other projects also answer `404`. A customer cannot reach a login form on a checkout domain.

An example with two shops on one server:

| Host name | Points to | Used for |
|---|---|---|
| `pay.example.com` | The server | Site URL. Checkout and API for the first project |
| `pay.shop-two.com` | The server | Checkout domain of the second project |

## Give a project a checkout domain

You need the `root` or `admin` role. These steps use the Caddy server from the install. With your own proxy, do steps 3 and 4 in your proxy.

1. Create DNS records for the name. Add an `A` record with the server's IPv4 address. Add an `AAAA` record if the server has IPv6.
2. Check the record from your computer:

   ```bash
   dig +short pay.shop-two.com
   ```

3. Add the name to the Caddy site address. In `/opt/turmite/.env`, add it to `TURMITE_CADDY_ADDRESS`, separated by a comma:

   ```dotenv
   TURMITE_CADDY_ADDRESS=pay.example.com, pay.shop-two.com
   ```

4. Start Caddy again with the new value:

   ```bash
   cd /opt/turmite
   docker compose up -d caddy
   ```

5. Check that the server answers on the new name. The command must print `ok`:

   ```bash
   curl -s https://pay.shop-two.com/healthz
   ```

6. In the dashboard, select the project and open **Settings → Domain**.
7. Enter `pay.shop-two.com` in **Host name**, without `https://`.
8. Enter a fresh code in **Two-factor code** and click **Save**.

Then check the result:

- `https://pay.shop-two.com/dashboard/login` answers `404`.
- The `payment_url` and `invoice_url` of the project's payments start with `https://pay.shop-two.com/`. This is true for earlier payments too.
- Links that you sent before the change still work on the site URL.

Last, set the shop's API base URL to `https://pay.shop-two.com`. The merchant API also works on the checkout domain.

When the site URL has a path, links on the checkout domain use the same path. To remove the checkout domain, save an empty **Host name**.

### Rules

- A checkout domain belongs to one project.
- The host of the site URL cannot be a checkout domain.
- Turmite matches the domain on the `Host` header. Your proxy must pass the original `Host`.

If you use Cloudflare, see [Reverse proxy, Cloudflare and firewall](/docs/reverse-proxy/#cloudflare) for TLS settings and certificates.

## Remove a project

Turmite cannot delete a project. To stop a project:

1. Wait for every open payment to reach a final status.
2. In **Wallets**, click **Stop taking payments on** each network.
3. Revoke the project's API keys.
4. Remove its webhook endpoints and its team members.
5. Remove its checkout domain from **Settings → Domain**, from Caddy and from DNS.
