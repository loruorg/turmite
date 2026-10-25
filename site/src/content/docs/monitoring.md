---
title: Monitoring and logs
description: Check the health endpoint, read the JSON logs, and alert on the events that need a person.
sidebarTitle: Monitoring and logs
---

Monitor three things: the `api` answers, the `worker` follows each chain, and the hot wallets have funds.

## Health check

`GET /healthz` returns `ok` with status `200` when the `api` process answers HTTP. It does not check the database or the nodes. A checkout domain also serves it.

```bash
curl -s https://pay.example.com/healthz
```

Point an external uptime monitor at this URL. Check it every minute.

Docker checks the containers too. `docker compose ps` shows the result:

| Service | Health check |
|---|---|
| `postgres` | `pg_isready` every 5 seconds |
| `api` | `turmite healthcheck` every 30 seconds, after a start period of 60 seconds. Unhealthy after 3 failures |
| `worker` | None. It opens no port |
| `caddy` | None |

Docker does not restart a container that is unhealthy. It restarts a container that stops (`restart: unless-stopped`). Watch the `worker` through the logs and the **Server → Networks** page.

## Logs

Turmite writes one JSON object per line to standard output when the output is not a terminal, as in Docker. Each line has `timestamp`, `level` (`TRACE`, `DEBUG`, `INFO`, `WARN` or `ERROR`), `target`, `message` and the fields of the event, for example `payment_id` or `network`. The default level is `info`.

```bash
cd /opt/turmite
sudo docker compose logs -f api worker
sudo docker compose logs --since 1h worker
```

To show only errors, use `jq`:

```bash
sudo docker compose logs --no-log-prefix --since 24h worker | jq -cR 'fromjson? | select(.level == "ERROR")'
```

Logs do not contain API keys, passwords, TOTP secrets or private keys. Node errors show the node URL without the user name, the password, the query and a long path, because providers often put the API key there.

On testnet without SMTP, Turmite writes each email to the log, at level `WARN`, with the message `smtp not set up; email written to the log`.

The compose file limits the logs of each service to 5 files of 10 MB.

## What to alert on

| Signal | Where to see it | Action |
|---|---|---|
| `/healthz` does not return `ok` | Uptime monitor | Read the `api` log. Check PostgreSQL |
| `"level":"ERROR"` in any log | Logs | Read the message. Most errors name the network or the payment |
| **No new block** or **Not connected** on a network | Server → Networks | Check the nodes. Add or replace a node. See [Networks and nodes](/docs/nodes/#when-a-network-falls-behind) |
| `network moved to another node` (`WARN`), `node.failover` in the audit log | Logs, Server → Audit log | A node failed. Check it |
| Hot wallet balance is low | Server → Hot wallets | Send funds from your cold wallet. Without gas, sweeps and payouts stop |
| **Funds wait at deposit addresses** | Project overview, payment page under **Sweeps to your wallet** | A sweep is not final after 30 minutes, or confirmed funds are not swept after 60 minutes. Read the reason, check the hot wallet balance and the node, and read the `worker` log |
| Webhook deliveries in **Dead** | Webhooks → Deliveries | Fix the endpoint, then click **Retry** |
| Payouts in `needs-review` | Payouts, and the `payout.needs-review` event | Check the transaction on a block explorer. See [Payouts](/docs/payouts/) |
| Update or advisory banner | Dashboard | Plan an upgrade. See [Upgrades](/docs/upgrades/) |
| Disk space | Server | Keep at least 20% free for PostgreSQL, the dumps and the logs |
| Certificate expiry | Uptime monitor with TLS checks | Read the Caddy log: `sudo docker compose logs caddy` |

The **No new block** badge shows when a network has no new block for 60 minutes on Bitcoin, 20 minutes on Monero and 5 minutes on the other networks.

## Node failover

The `worker` probes every enabled node of each network every 30 seconds. The `api` probes them every 120 seconds. A probe reads the block height of the node, with a timeout of 15 seconds. All networks report a height. Monero reports none while its wallet scans, or until a project has a Monero wallet. The probe then only checks that the node answers.

- A node that is far behind the best node of its network counts as failed: more than 2 blocks on Bitcoin, 3 on Monero, 10 on Ethereum, 60 on Base and Polygon, 160 on BNB Smart Chain, 300 on Solana and 40 on Tron and TON.
- After 2 failed probes in a row, the network moves to the next healthy node, in the order that you added the nodes.
- When a node of higher priority passes 2 probes in a row, the network moves back to it.

The **Networks** page marks the node in use with **In use**. Each move writes a `node.failover` entry to the audit log.

## Events to your own tools

A project webhook endpoint can feed your own alerting. Add an endpoint for your monitoring tool and react to these events:

- `payout.needs-review`;
- `payout.failed`.

See [Webhooks](/docs/webhooks/) for the body and signatures.

## Audit log

Read **Server → Audit log** each week. Look for sign-ins from unknown IP addresses, new API keys, wallet changes, new nodes and address book changes.

## Resource use

```bash
sudo docker stats --no-stream
sudo docker compose exec postgres psql -U turmite turmite -c "select pg_size_pretty(pg_database_size('turmite'));"
```

Run self-hosted chain nodes on other machines. They need much more CPU, memory and disk than Turmite.
