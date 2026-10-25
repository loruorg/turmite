---
title: Run under a path
description: Serve Turmite under a path of an existing site, such as https://example.com/turmite, behind your own reverse proxy.
---

You can run Turmite beside another app on the same host. Put the path in the site URL. Your own reverse proxy keeps port 443 and the certificate, and passes the path to Turmite.

```dotenv
TURMITE_SITE_URL=https://example.com/turmite
```

## What follows the site URL

Every URL that Turmite makes starts with the site URL:

| What | URL |
|---|---|
| Dashboard | `https://example.com/turmite/dashboard` |
| Invoice page | `https://example.com/turmite/i/{token}` |
| Payment page | `https://example.com/turmite/pay/{token}` |
| Merchant API | `https://example.com/turmite/v1/payment` |
| Static files | `https://example.com/turmite/assets/` |
| Health check | `https://example.com/turmite/healthz` |

Your shop uses `https://example.com/turmite` as the API base URL.

A project with a [checkout domain](/docs/projects/#give-a-project-a-checkout-domain) uses the same path on its own host, for example `https://pay.shop-two.com/turmite/pay/{token}`.

## Install with a path

1. Run the install script with the full URL and `--no-start`:

   ```bash
   curl -fsSL https://turmite.com/install.sh | sudo sh -s -- --site-url https://example.com/turmite --no-start
   ```

   With a path in the site URL, `turmite init` sets `TURMITE_CADDY_ADDRESS=http://:80` in `.env`. The bundled Caddy would serve plain HTTP and get no certificate.

2. Your own proxy already uses ports 80 and 443. Turn off the bundled Caddy and publish the api port on the loopback interface. Write `/opt/turmite/docker-compose.override.yml`:

   ```yaml
   services:
     api:
       ports:
         - "127.0.0.1:8080:8080"
     caddy:
       profiles: [disabled]
   ```

3. Start the stack:

   ```bash
   cd /opt/turmite && sudo docker compose up -d
   ```

Docker Compose reads the override file automatically. `install.sh --upgrade` does not change it.

## nginx

Add this to the `server` block that serves `example.com`:

```nginx
location = /turmite { return 301 /turmite/; }

location /turmite/ {
    proxy_pass http://127.0.0.1:8080;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-For $remote_addr;
    client_max_body_size 4m;
}
```

Write `proxy_pass` without a path. nginx then passes `/turmite/...` unchanged, and Turmite removes the prefix itself. Turmite also answers requests without the prefix, but the links that it makes always contain the prefix.

## Caddy

```text
example.com {
	handle /turmite/* {
		reverse_proxy 127.0.0.1:8080
	}
	# your other app
}
```

Use `handle`. Do not use `handle_path`, because it removes the prefix.

## Trusted proxy

The api sees a proxy on the same host at the address of the Docker network gateway. That address is in the default `TURMITE_TRUSTED_PROXIES` list. If your proxy runs on another machine, set the list. See [Client IP addresses](/docs/reverse-proxy/#client-ip-addresses).

## Cookies and the origin check

- The session cookie `turmite_session` has `Path=/turmite`. Your main app does not receive it.
- Each dashboard `POST` must carry an `Origin` (or `Referer`) header with the origin of the site URL, here `https://example.com`. Do not let your proxy change `Origin`.

## Security of a shared host

A path does not separate two apps in the browser. Your main app and Turmite have the same origin. A cross-site scripting bug in the main app lets a script act in the dashboard while an admin is signed in. The cookie path does not stop this.

Turmite still asks for a fresh TOTP code for payouts, API keys and other sensitive actions. Use a path only beside an app that you trust as much as Turmite. In other cases, use a subdomain such as `pay.example.com`.

## Put the dashboard behind your own login

Customers, your shop and the exchange need only these paths:

| Path | Used by |
|---|---|
| `/turmite/i/` | Invoice pages |
| `/turmite/pay/` | Payment pages and the status frame |
| `/turmite/x/` | Pay with any coin: exchange pages, status frame and the exchange webhook |
| `/turmite/v1/` | Your shop's server |
| `/turmite/assets/`, `/turmite/favicon.svg` | Checkout styles and images |
| `/turmite/branding/` | Project logos |
| `/turmite/healthz` | Uptime checks |

Protect everything else under the prefix. For example, with nginx:

```nginx
location /turmite/ {
    auth_request /your-admin-auth;
    proxy_pass http://127.0.0.1:8080;
    # the same proxy_set_header lines as above
}

location ~ ^/turmite/(i/|pay/|x/|v1/|assets/|branding/|healthz$|favicon\.svg$) {
    proxy_pass http://127.0.0.1:8080;
    # the same proxy_set_header lines as above
}
```

nginx uses the regular expression location for the public paths, because it takes priority over the prefix location. Turmite's own login and two-factor checks still apply. For other ways to protect the dashboard, see [Private dashboard](/docs/private-dashboard/).
