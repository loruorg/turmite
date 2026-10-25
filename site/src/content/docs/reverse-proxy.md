---
title: Reverse proxy, Cloudflare and firewall
description: Put Turmite behind Caddy, nginx or Cloudflare, keep the real client IP, and close every other port.
sidebarTitle: Reverse proxy and firewall
---

The `api` process speaks plain HTTP on port 8080. A reverse proxy in front of it handles TLS. The install script sets up Caddy for this. The `worker` opens no port.

## Caddy (default)

The install script starts Caddy with automatic TLS. Caddy gets a certificate for the host of the site URL and renews it. Ports 80 and 443 must be open to the internet for this. The site address comes from `TURMITE_CADDY_ADDRESS` in `.env`. `turmite init` sets it to the host of the site URL.

The main parts of `/opt/turmite/Caddyfile`:

```text
{
	servers {
		trusted_proxies static {$TURMITE_TRUSTED_PROXIES:private_ranges}
	}
}

(turmite) {
	encode zstd gzip
	request_body {
		max_size 4MB
	}
	header ?Strict-Transport-Security "max-age=31536000"
	header ?X-Content-Type-Options "nosniff"
	header ?Referrer-Policy "same-origin"
	header ?Permissions-Policy "camera=(), microphone=(), geolocation=(), payment=(), usb=()"
	header ?Cross-Origin-Opener-Policy "same-origin"
	header -Server
	header -Via
	reverse_proxy api:8080 {
		header_up -Forwarded
	}
}

{$TURMITE_CADDY_ADDRESS} {
	import turmite
}
```

Turmite sets its own content security policy, frame, cache and referrer headers per page. On an `https://` site URL it also sends `Strict-Transport-Security: max-age=31536000`. The `?` headers in Caddy apply only when Turmite did not set the header.

After you change the Caddyfile, restart Caddy:

```bash
cd /opt/turmite
sudo docker compose restart caddy
```

`install.sh --upgrade` keeps your Caddyfile. If a release changes it, the script writes `Caddyfile.new` next to it.

For a project's checkout domain, see [Give a project a checkout domain](/docs/projects/#give-a-project-a-checkout-domain).

## Your own proxy

You can use another proxy, such as nginx. The compose file does not publish the api port. Publish it on the loopback interface, and turn off the bundled Caddy, in `/opt/turmite/docker-compose.override.yml`:

```yaml
services:
  api:
    ports:
      - "127.0.0.1:8080:8080"
  caddy:
    profiles: [disabled]
```

Docker Compose reads this file automatically. `install.sh --upgrade` does not change it. On a new install, run the install script with `--no-start`, write the file, then run `sudo docker compose up -d`.

Point the proxy at the api, and pass the `Host` header and the client IP:

```nginx
server {
    listen 443 ssl;
    server_name pay.example.com;
    # ssl_certificate and ssl_certificate_key lines here

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $remote_addr;
        client_max_body_size 4m;
    }
}
```

Never publish port 8080 on a public interface.

Rules for every proxy:

- Pass the original `Host` header. Turmite uses it to find [checkout domains](/docs/projects/#give-a-project-a-checkout-domain).
- Do not change the `Origin` header. The dashboard refuses a `POST` whose `Origin` does not match the site URL.
- Allow request bodies of at least 4 MB. A logo upload can be 2 MB, plus the form data.

To serve Turmite under a path of another site, see [Run under a path](/docs/path-prefix/).

## Client IP addresses

Turmite uses the client IP for rate limits and the audit log. The limits are 10 sign-in attempts a minute per IP address and per email address, 600 merchant API requests a minute per IP address, and 300 checkout requests a minute per IP address.

Turmite reads `X-Forwarded-For` only when the connection comes from an address in `TURMITE_TRUSTED_PROXIES`. It then uses the rightmost address in the header that is not a trusted proxy. The default list is loopback and the private ranges, which include the Docker network that Caddy uses.

The bundled Caddy reads the same variable for its own list of trusted proxies. If a proxy connects from a public address, set the list in `.env`. The list replaces the default, so add the private ranges again. Separate the ranges with spaces, because Caddy does not accept commas:

```dotenv
TURMITE_TRUSTED_PROXIES=10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 203.0.113.7/32
```

Then run `sudo docker compose up -d` in `/opt/turmite`.

If the setting is wrong, all visitors look like one IP address. Then one person can lock everyone out of sign-in for a minute, and busy checkouts hit the limit.

## Cloudflare

Cloudflare's proxy can sit in front of the server. Visitors then see Cloudflare addresses, and you can block paths before they reach the server.

1. Add the domain to Cloudflare.
2. In **DNS → Records**, add the `A` record (and `AAAA` if you have IPv6) with **Proxy status: Proxied**.
3. In **SSL/TLS → Overview**, set the mode to **Full (strict)**. Do not use **Flexible**. It sends traffic to your server without encryption and causes redirect loops.
4. Trust Cloudflare's addresses. See the next section.
5. Restrict ports 80 and 443 to Cloudflare's addresses. See [Firewall](#firewall).

### Keep the real visitor IP

Behind Cloudflare, every request comes from a Cloudflare address. Cloudflare adds the visitor's address to `X-Forwarded-For`. Add Cloudflare's ranges to `TURMITE_TRUSTED_PROXIES`. Caddy then keeps the header, and Turmite takes the visitor's address from it.

```bash
cf=$({ curl -fsS https://www.cloudflare.com/ips-v4; echo; curl -fsS https://www.cloudflare.com/ips-v6; } | xargs)
echo "TURMITE_TRUSTED_PROXIES=10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 $cf" | sudo tee -a /opt/turmite/.env
cd /opt/turmite && sudo docker compose up -d
```

Keep only one `TURMITE_TRUSTED_PROXIES` line in `.env`. Cloudflare changes its ranges rarely. Run the command again from time to time, and remove the old line.

### Certificates behind Cloudflare

Caddy still gets its own certificate behind Cloudflare with **Full (strict)**. Cloudflare challenges and "Under Attack" mode can block the certificate check. The certificate then fails to renew, and visitors see error 525 or 526.

A Cloudflare Origin Certificate avoids this. Only Cloudflare trusts it, which is enough behind the proxy.

1. In Cloudflare, open **SSL/TLS → Origin Server → Create Certificate**.
2. List the host names, for example `pay.example.com`.
3. Save the certificate as `/opt/turmite/origin.pem`.
4. Save the private key as `/opt/turmite/origin-key.pem`. Cloudflare shows the key one time only.
5. Run `sudo chmod 600 /opt/turmite/origin-key.pem`.
6. Add a `tls` line to the site block of the Caddyfile:

   ```text
   {$TURMITE_CADDY_ADDRESS} {
   	import turmite
   	tls /etc/caddy/origin.pem /etc/caddy/origin-key.pem
   }
   ```

7. Mount the files into the Caddy container, in `/opt/turmite/docker-compose.override.yml`:

   ```yaml
   services:
     caddy:
       volumes:
         - ./origin.pem:/etc/caddy/origin.pem:ro
         - ./origin-key.pem:/etc/caddy/origin-key.pem:ro
   ```

8. Run `sudo docker compose up -d caddy`.

Each Cloudflare zone needs its own certificate. A checkout domain in a different zone, such as `pay.shop-two.com`, needs a second certificate and its own site block.

### Bot protection and your shop

Cloudflare Bot Fight Mode and a high Security Level can challenge requests that do not come from a browser. Your shop's server then gets `403` from `/v1/`. To fix this, add a WAF custom rule with the action **Skip** for paths that start with `/v1/`. Or turn off Bot Fight Mode for the zone.

Webhooks leave your server directly. Cloudflare does not touch them.

## Firewall

Open only these ports:

| Port | From | Use |
|---|---|---|
| 22 | Your IP addresses | SSH |
| 80, 443 | Anywhere, or only Cloudflare's ranges when you use Cloudflare | HTTP and HTTPS |

Never publish PostgreSQL (5432) or the api port (8080). The compose file keeps PostgreSQL on an internal network with no route to the internet.

### Docker bypasses ufw

Docker writes its own iptables rules for published ports. These rules come before the ufw rules. A ufw rule does not stop traffic to a port that Docker publishes.

Use one of these:

- The firewall of your hosting provider. It filters traffic before it reaches the server, so Docker cannot change it. This is the better choice.
- The `DOCKER-USER` iptables chain. Docker reads this chain before its own rules.

Example: let only Cloudflare reach ports 80 and 443.

```bash
for r in $(curl -fsS https://www.cloudflare.com/ips-v4); do
  sudo iptables -I DOCKER-USER -i eth0 -s "$r" -p tcp -m multiport --dports 80,443 -j RETURN
done
sudo iptables -A DOCKER-USER -i eth0 -p tcp -m multiport --dports 80,443 -j DROP
sudo apt install iptables-persistent
sudo netfilter-persistent save
```

Replace `eth0` with your public interface. `ip route get 1.1.1.1` shows it. Do the same with `ip6tables` for IPv6. Caddy also listens on UDP 443 for HTTP/3. Add the same rules for `-p udp` to filter it too.

### Check the firewall

From another machine, connect to the server IP directly. Behind Cloudflare, this command must time out:

```bash
curl -sk --max-time 10 --resolve pay.example.com:443:198.51.100.10 https://pay.example.com/healthz
```

Replace `198.51.100.10` with the server's public IP address.

## Next

To keep the dashboard off the public internet, see [Private dashboard](/docs/private-dashboard/).
