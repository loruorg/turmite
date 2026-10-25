---
title: Accept your first payment
description: Create an API key, create an invoice, send the customer to checkout and mark the order paid from the webhook.
sidebarTitle: First payment
---

This page connects a shop to Turmite. You create an API key, create an invoice, send the customer to the checkout page and mark the order paid from the webhook. Do it on a testnet install first.

## Before you start

- A running install. See [Install on a server](/docs/install/).
- At least one network with a node and a wallet. See [Networks and nodes](/docs/nodes/) and [Wallets per chain](/docs/wallets/).
- A user with the `project_admin` role. On a mainnet install, the user needs two-factor authentication.

The examples use `https://pay.example.com` for Turmite and `https://shop.example.com` for the shop.

## 1. Create an API key

1. In the dashboard, open **API keys**.
2. Under **Create a key**, enter a label and your two-factor code. Click **Create key**.
3. Copy the key. It starts with `tm_`. The dashboard shows it once.
4. Store it in the shop's server environment as `TM_API_KEY`.

Keep the key on the server. Never put it in browser code. A key works only for its own project.

## 2. Copy the IPN secret

Turmite signs each webhook with the project's IPN secret.

1. In the dashboard, open **Settings**.
2. Under **IPN secret**, enter your two-factor code and click **Show secret**.
3. Copy the secret.
4. Store it in the shop's server environment as `TM_IPN_SECRET`.

To replace the secret later, click **Rotate** under **IPN secret**.

## 3. Check the connection

```bash
curl https://pay.example.com/v1/status
```

The response is `{"message":"OK"}`. Then list the tickers that your project accepts:

```bash
curl https://pay.example.com/v1/currencies -H "x-api-key: $TM_API_KEY"
```

## 4. Create an invoice

Create an invoice when the customer checks out. The customer chooses the coin on the Turmite checkout page.

```bash
curl https://pay.example.com/v1/invoice \
  -H "x-api-key: $TM_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "price_amount": 25,
    "price_currency": "usd",
    "order_id": "ORDER-1001",
    "order_description": "Order 1001",
    "ipn_callback_url": "https://shop.example.com/turmite/ipn",
    "success_url": "https://shop.example.com/orders/ORDER-1001",
    "cancel_url": "https://shop.example.com/cart"
  }'
```

The same request in Node.js 18 or later:

```js
const res = await fetch('https://pay.example.com/v1/invoice', {
  method: 'POST',
  headers: {
    'x-api-key': process.env.TM_API_KEY,
    'content-type': 'application/json',
  },
  body: JSON.stringify({
    price_amount: 25,
    price_currency: 'usd',
    order_id: 'ORDER-1001',
    order_description: 'Order 1001',
    ipn_callback_url: 'https://shop.example.com/turmite/ipn',
    success_url: 'https://shop.example.com/orders/ORDER-1001',
    cancel_url: 'https://shop.example.com/cart',
  }),
});
if (!res.ok) throw new Error(`Turmite returned ${res.status}`);
const invoice = await res.json();
```

In PHP:

```php
$ch = curl_init('https://pay.example.com/v1/invoice');
curl_setopt_array($ch, [
    CURLOPT_POST => true,
    CURLOPT_RETURNTRANSFER => true,
    CURLOPT_TIMEOUT => 20,
    CURLOPT_HTTPHEADER => [
        'x-api-key: ' . getenv('TM_API_KEY'),
        'Content-Type: application/json',
    ],
    CURLOPT_POSTFIELDS => json_encode([
        'price_amount' => 25,
        'price_currency' => 'usd',
        'order_id' => 'ORDER-1001',
        'order_description' => 'Order 1001',
        'ipn_callback_url' => 'https://shop.example.com/turmite/ipn',
        'success_url' => 'https://shop.example.com/orders/ORDER-1001',
        'cancel_url' => 'https://shop.example.com/cart',
    ]),
]);
$invoice = json_decode(curl_exec($ch), true);
```

In Python:

```python
import os, requests

r = requests.post(
    "https://pay.example.com/v1/invoice",
    headers={"x-api-key": os.environ["TM_API_KEY"]},
    json={
        "price_amount": 25,
        "price_currency": "usd",
        "order_id": "ORDER-1001",
        "order_description": "Order 1001",
        "ipn_callback_url": "https://shop.example.com/turmite/ipn",
        "success_url": "https://shop.example.com/orders/ORDER-1001",
        "cancel_url": "https://shop.example.com/cart",
    },
    timeout=20,
)
r.raise_for_status()
invoice = r.json()
```

The response has the checkout link in `invoice_url`:

```json
{
  "id": "4522625843",
  "token_id": "k3J9xQ2vT8mZ5wR1nB7cY4pL",
  "order_id": "ORDER-1001",
  "order_description": "Order 1001",
  "price_amount": "25",
  "price_currency": "usd",
  "pay_currency": null,
  "ipn_callback_url": "https://shop.example.com/turmite/ipn",
  "invoice_url": "https://pay.example.com/i/k3J9xQ2vT8mZ5wR1nB7cY4pL",
  "success_url": "https://shop.example.com/orders/ORDER-1001",
  "cancel_url": "https://shop.example.com/cart",
  "created_at": "2026-10-23T12:00:00.000Z",
  "updated_at": "2026-10-23T12:00:00.000Z",
  "expiration_estimate_date": "2026-10-24T12:00:00.000Z"
}
```

`id` is numeric text and `price_amount` is a decimal string. Save `id` with the order.

## 5. Send the customer to checkout

Redirect the customer to `invoice_url`. The checkout page:

1. Shows the amount and the coins that the project accepts.
2. Creates a payment when the customer chooses a coin.
3. Shows the deposit address, the exact amount, a QR code and a countdown.
4. Updates the status every few seconds. It works with JavaScript off.
5. Shows a button back to `success_url` when the payment is complete.

The return button is only a link. Do not mark the order paid when the customer visits `success_url`. Use the webhook in step 6.

See [Checkout](/docs/checkout/) for branding and the return URL.

### Use your own payment page

To choose the coin in your shop, call `POST /v1/payment` with `pay_currency`. The response has `pay_address` and `pay_amount`. Your shop shows them to the customer, or sends the customer to `payment_url`, the hosted payment page for that coin. See [Merchant API](/docs/merchant-api/).

## 6. Handle the webhook

Turmite sends a `POST` to `ipn_callback_url` at each status change. The body is the payment object. The `x-turmite-sig` header holds the hex HMAC-SHA512 of the body with keys sorted, signed with the IPN secret.

An example body:

```json
{
  "payment_id": 5524759814,
  "invoice_id": 4522625843,
  "payment_status": "finished",
  "pay_address": "tb1q6rz28mcfaxtmd6v789l9rrlrusdprr9pz3cppk",
  "payin_extra_id": null,
  "price_amount": 25,
  "price_currency": "usd",
  "pay_amount": 0.00031245,
  "actually_paid": 0.00031245,
  "pay_currency": "btc",
  "order_id": "ORDER-1001",
  "order_description": "Order 1001",
  "purchase_id": "6084744717",
  "outcome_amount": 0.00031245,
  "outcome_currency": "btc",
  "network": "btc",
  "network_precision": 8,
  "smart_contract": null,
  "payin_hash": "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b",
  "payout_hash": null,
  "payment_url": "https://pay.example.com/pay/Q7mX2pK9vT4nB8cR1sW6yZ3h",
  "created_at": "2026-10-23T12:00:00.000Z",
  "updated_at": "2026-10-23T12:41:07.000Z",
  "expiration_estimate_date": "2026-10-23T13:00:00.000Z"
}
```

In the webhook, `payment_id` and `invoice_id` are numbers and `purchase_id` is a string. Amounts are numbers. `network` is a lowercase code.

A handler in Node.js with Express and the [TypeScript SDK](/docs/sdk/). `verifyIpnSignature` makes the key-sorted JSON from the raw body and keeps the text of each number:

```js
import express from 'express';
import { isPaid, verifyIpnSignature } from '@turmite/sdk';

const app = express();

app.post('/turmite/ipn', express.raw({ type: 'application/json' }), async (req, res) => {
  const ok = await verifyIpnSignature(req.body, req.get('x-turmite-sig'), process.env.TM_IPN_SECRET);
  if (!ok) return res.sendStatus(401);

  const payment = JSON.parse(req.body.toString('utf8'));
  if (isPaid(payment.payment_status)) {
    await markOrderPaid(payment.order_id, payment.payment_id); // must be idempotent
  }
  res.sendStatus(200);
});
```

PHP, Python, Go and Ruby versions, and the timestamped `x-turmite-signature`, are in [Webhooks](/docs/webhooks/).

Rules for the handler:

- Verify the signature before you read any field.
- Ship the order only on `finished`.
- Check that `order_id` and `invoice_id` match an order that you created.
- Make the handler idempotent. The same status can arrive more than once.
- Respond with a `2xx` status fast. Do slow work, for example emails, after the response.
- Do not depend on the order of deliveries. Turmite does not guarantee it. Use `payment_status` and `updated_at` from the body.
- Turmite waits 30 seconds for each response. A slower response counts as a failure and Turmite retries.

See [Payment statuses](/docs/statuses/) for every status and what to do.

## 7. Check the status without a webhook

If your shop missed a webhook, ask for the payment:

```bash
curl https://pay.example.com/v1/payment/5524759814 -H "x-api-key: $TM_API_KEY"
```

Use this as a fallback only. Do not poll in a tight loop.

## 8. Test on testnet

1. On a testnet install, enable one network and set its wallet. Base Sepolia or Solana Devnet are fast.
2. Get test coins from a public faucet for that network. On Solana Devnet you can run `solana airdrop 1 <address> --url devnet`.
3. Create an invoice and open `invoice_url`.
4. Pay the exact amount from your test wallet.
5. Watch the payment in the dashboard under **Payments**.
6. Check that your handler received each status change up to `finished`.

On testnet, `ipn_callback_url` can be an `http://` URL or a private address. On mainnet it must be a public `https://` URL.

To test the handler without real coins, run `turmite serve --demo` on your computer. Create a payment, open it in the dashboard and use **Simulate a deposit**. Turmite then sends the IPN as for a real deposit. See [Quick start](/docs/quick-start/).

## Next steps

- [Webhooks](/docs/webhooks/): project endpoints, retries and the second signature.
- [Payouts](/docs/payouts/): refunds and supplier payments from the hot wallet.
- [Go-live checklist](/docs/go-live/): before you switch the shop to mainnet.
