---
title: Private dashboard
description: Keep the checkout and the merchant API public, and make the dashboard reachable only for your team.
---

Customers need the checkout. Your shop needs the merchant API. Only your team needs the dashboard, and the dashboard is where money moves. Block the dashboard on every public host name. Then reach it through a private entrance.

Two-factor codes still apply. These steps reduce who can try a password at all.

## Public and private paths

| Path | Who needs it |
|---|---|
| `/i/`, `/pay/` | Customers (checkout) |
| `/x/` | Customers and the exchange (pay with any coin) |
| `/v1/` | Your shop's server (merchant API) |
| `/assets/`, `/branding/`, `/favicon.svg` | Customers (checkout styles, images and project logos) |
| `/healthz` | Uptime monitors |
| `/dashboard` and everything under it | Only your team |

`/` redirects to `/dashboard`.

With a path in the site URL, add the prefix. For example, `/turmite/dashboard`.

A [checkout domain](/docs/projects/#give-a-project-a-checkout-domain) serves only the paths above, without `/dashboard`. It answers `404` for every other path. The steps below are for the host of the site URL.

## Option 1: Tailscale

[Tailscale](https://tailscale.com) makes a private network between your devices and the server. The dashboard gets an address that only your devices can open.

1. Install Tailscale on the server and sign in:

   ```bash
   curl -fsSL https://tailscale.com/install.sh | sh
   sudo tailscale up
   ```

2. Install Tailscale on your laptop and phone. Sign in with the same account.
3. In the Tailscale admin console, turn on MagicDNS and HTTPS certificates.
4. Run `tailscale status` on the server. Note its name, for example `turmite.example-tailnet.ts.net`.
5. Add a private entrance to the Caddyfile. Replace both host names with your own:

   ```text
   http://:8081 {
   	reverse_proxy api:8080 {
   		header_up Host pay.example.com
   		header_up Origin ^https://turmite\.example-tailnet\.ts\.net$ https://pay.example.com
   	}
   }
   ```

6. Publish port 8081 on the loopback interface only, in `docker-compose.override.yml`:

   ```yaml
   services:
     caddy:
       ports:
         - "127.0.0.1:8081:8081"
   ```

7. Start Caddy, then give the port to Tailscale:

   ```bash
   cd /opt/turmite
   sudo docker compose up -d caddy
   sudo tailscale serve --bg http://127.0.0.1:8081
   ```

8. Open `https://turmite.example-tailnet.ts.net/dashboard/login` from a device on your tailnet.

Docker Compose reads `docker-compose.override.yml` automatically. `install.sh --upgrade` keeps it and your Caddyfile.

Turmite accepts a dashboard `POST` only when its `Origin` matches the origin of the site URL. The entrance sends the site URL's host to Turmite. The `Origin` line changes only requests that come from your tailnet name. Do not replace it with a fixed value. A fixed value turns off the cross-site check for this entrance.

If the `.ts.net` name does not resolve on your laptop, turn on Tailscale DNS there: `sudo tailscale set --accept-dns=true`.

### Alternative: a site URL on the tailnet

You can also set `TURMITE_SITE_URL` to the tailnet name and give every project a checkout domain. Payment and invoice links then use the checkout domain of the project. A checkout domain serves the checkout and the merchant API, and never the dashboard. Webhooks leave the server directly, so they still work.

This setup needs more changes:

- Caddy cannot get a public certificate for a `.ts.net` name. Serve the site URL through `tailscale serve`, and give Caddy a site block for each checkout domain.
- Turmite gives the exchange a webhook URL on the site URL, which the exchange cannot reach. The worker also polls each swap every 10 seconds, so swaps still complete.

## Option 2: Cloudflare Access

Cloudflare Access (Zero Trust) asks for a team sign-in before a request reaches the server.

1. In Cloudflare Zero Trust, open **Access → Applications**.
2. Add a self-hosted application.
3. Set the domain to `pay.example.com` and the path to `dashboard`.
4. Add a policy that allows only your team's email addresses.
5. Save the application.

The checkout and the merchant API stay open, because the application covers only `/dashboard`.

## Option 3: Cloudflare WAF rule

A WAF custom rule blocks the dashboard on the public host names.

1. In Cloudflare, open the domain, then **Security → WAF → Custom rules**.
2. Select **Create rule**, then **Edit expression**.
3. Paste this expression:

   ```text
   (http.host in {"pay.example.com"} and starts_with(http.request.uri.path, "/dashboard"))
   ```

4. Set the action to **Block**.
5. Deploy the rule.

To keep the dashboard open from your office, add your addresses before the last bracket:

```text
(http.host in {"pay.example.com"} and starts_with(http.request.uri.path, "/dashboard") and not ip.src in {203.0.113.10})
```

Add each public host name to the `{...}` list, separated by spaces. A host in another Cloudflare zone needs the same rule in that zone.

## Option 4: block on the server

A Cloudflare rule protects only traffic that goes through Cloudflare. To make the server refuse the dashboard on public names, add two lines to each public site block in the Caddyfile:

```text
{$TURMITE_CADDY_ADDRESS} {
	import turmite
	@dashboard path /dashboard /dashboard/*
	respond @dashboard 404
}
```

Then run `sudo docker compose restart caddy`.

Do not add these lines to the private entrance on port 8081. If you do, you lock yourself out.

## Recommended setup

Use Tailscale to sign in. Use a WAF rule or the Caddy block on every public host name. Use the Caddy block when you do not use Cloudflare.

## Check the result

From a browser that is not on your tailnet:

1. `https://pay.example.com/dashboard/login` is blocked or returns `404`.
2. `https://pay.example.com/healthz` returns `ok`.
3. A payment link from your shop opens the checkout.

From a device on your tailnet:

1. The dashboard opens on the private entrance.
2. You can sign in and save a setting.
