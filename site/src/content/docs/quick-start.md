---
title: Quick start
description: Run Turmite on your computer in demo mode with Docker Compose and create a test payment.
---

This page starts Turmite on your computer in demo mode. Demo mode uses mock chains. No real coins move and you need no nodes or wallets.

Do not use demo mode on a public server. To install Turmite for real payments, see [Install on a server](/docs/install/).

## Requirements

- Docker with the Compose plugin (`docker compose version` prints a version).
- `openssl` and `curl`.
- Port 8080 free on your computer.

## 1. Write the Compose file

Make an empty directory and go into it:

```bash
mkdir turmite-demo && cd turmite-demo
```

Make a server secret. Compose reads it from `.env`:

```bash
echo "TURMITE_SECRET=$(openssl rand -hex 32)" > .env
```

Write this file as `compose.yml`:

```yaml
services:
  postgres:
    image: postgres:18
    environment:
      POSTGRES_USER: turmite
      POSTGRES_PASSWORD: turmite
      POSTGRES_DB: turmite
    volumes:
      - pgdata:/var/lib/postgresql
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U turmite -d turmite"]
      interval: 5s
      timeout: 5s
      retries: 10

  turmite:
    image: loruorg/turmite:latest
    command: ["serve", "--demo"]
    environment:
      TURMITE_DATABASE_URL: postgres://turmite:turmite@postgres:5432/turmite
      TURMITE_SITE_URL: http://localhost:8080
      TURMITE_SECRET: ${TURMITE_SECRET}
      TURMITE_MODE: testnet
    ports:
      - "127.0.0.1:8080:8080"
    depends_on:
      postgres:
        condition: service_healthy

volumes:
  pgdata:
```

The image runs the `turmite` binary, so `command` holds only the subcommand. Demo mode needs no `TURMITE_HOT_*` variables and always runs as testnet. Without `TURMITE_SECRET`, demo mode makes a random secret at each start. Then the sealed data, such as two-factor secrets and the IPN secret, is unreadable after a restart. The fixed secret in `.env` prevents this.

## 2. Start it

```bash
docker compose up -d
docker compose logs -f turmite
```

The `turmite` service runs migrations at start. Wait for the message `demo mode: open http://localhost:8080/dashboard to create the first account`. Then press `Ctrl+C` to stop following the log.

Demo mode makes a project called "Demo shop" with a mock wallet on every network. A banner in the dashboard says that the chains are simulated.

Check that the server answers:

```bash
curl http://localhost:8080/healthz
```

The response is `ok`.

## 3. Create the root user

1. Open `http://localhost:8080/dashboard`. With no user in the database, it opens the setup page.
2. Enter an email address and a password of 12 characters or more, two times. This user is the root user.
3. Submit the form. Turmite signs you in.
4. Optional: open the account page and set up two-factor authentication with an authenticator app.

With two-factor on, Turmite asks for a fresh code before sensitive actions, for example to create an API key. On a testnet install, you can do these actions without two-factor. On mainnet, two-factor must be on first.

## 4. Create an API key

1. In the dashboard, open **API keys**.
2. Under **Create a key**, enter a label. Enter your two-factor code if two-factor is on. Click **Create key**.
3. Copy the key. It starts with `tm_`. The dashboard shows it once.

Put the key in your shell:

```bash
export TM_API_KEY=tm_3f9a...
```

List the tickers that the project accepts:

```bash
curl -s http://localhost:8080/v1/currencies -H "x-api-key: $TM_API_KEY"
```

## 5. Create an invoice

```bash
curl -s http://localhost:8080/v1/invoice \
  -H "x-api-key: $TM_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "price_amount": 25,
    "price_currency": "usd",
    "order_id": "ORDER-1001",
    "order_description": "Demo order",
    "success_url": "http://localhost:8080/",
    "cancel_url": "http://localhost:8080/"
  }'
```

The response has an `invoice_url`. Open it in a browser:

1. Choose a coin. Turmite creates a payment and opens the payment page.
2. The payment page shows the amount, the deposit address, a QR code and a countdown.
3. The status box reloads every 4 seconds. It works with JavaScript off.

## 6. Simulate a payment

In demo mode, you add the deposits by hand. The deposit address of a demo payment looks like `mock-bitcoin-00000001`.

1. In the dashboard, open **Payments** and select the payment.
2. Under **Simulate a deposit**, keep the amount. It is the amount due.
3. Set **Confirmations** to `0` and click **Send**. The payment goes to `confirming`. The checkout page shows "Payment received".
4. Open the next payment, or create one, and send the amount with **Confirmations** set to `10`. That is enough on every test network. The payment goes to `confirmed` and then `finished`. The checkout page shows "Paid".

Send half of the amount to see `partially_paid`. Then send the rest to see `finished`.

You can also create a payment directly, without an invoice:

```bash
curl -s http://localhost:8080/v1/payment \
  -H "x-api-key: $TM_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "price_amount": 25,
    "price_currency": "usd",
    "pay_currency": "btc",
    "order_id": "ORDER-1002",
    "order_description": "Demo order"
  }'
```

List the payments and their status:

```bash
curl -s "http://localhost:8080/v1/payment?limit=5" -H "x-api-key: $TM_API_KEY"
```

The dashboard shows the same payment under **Payments**.

## 7. Stop and remove it

```bash
docker compose down -v
```

The `-v` flag deletes the database volume.

## Next steps

- [Install on a server](/docs/install/) for testnet or mainnet.
- [Accept your first payment](/docs/first-payment/) to connect a shop.
- [Merchant API](/docs/merchant-api/) for every endpoint.
