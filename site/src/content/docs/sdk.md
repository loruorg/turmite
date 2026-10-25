---
title: TypeScript SDK
description: Call the merchant API and verify webhooks from TypeScript or JavaScript with the @turmite/sdk package.
sidebarTitle: TypeScript SDK
---

`@turmite/sdk` is the TypeScript client for the merchant API. It also verifies webhooks. It has no runtime dependencies. The source is in `sdk/typescript` in the repository.

## Install

```bash
npm install @turmite/sdk
```

The SDK uses `fetch` and Web Crypto. It runs on Node.js 20 or later, Deno, Bun and Cloudflare Workers. It is an ES module.

Use the SDK on your server only. The API key must not reach a browser.

## Create a client

```ts
import { Turmite } from '@turmite/sdk';

const turmite = new Turmite({
  baseUrl: 'https://pay.example.com',
  apiKey: process.env.TM_API_KEY!,
});
```

| Option | Description |
|---|---|
| `baseUrl` | Your site URL, with its path if it has one. A checkout domain also works |
| `apiKey` | The project API key (`tm_…`) |
| `fetch` | A replacement for the global `fetch` |
| `timeoutMs` | Request timeout in milliseconds. The default is 30000 |

## Create an invoice

```ts
const invoice = await turmite.createInvoice({
  price_amount: 25,
  price_currency: 'usd',
  order_id: 'ORDER-1001',
  order_description: 'Order 1001',
  ipn_callback_url: 'https://shop.example.com/turmite/ipn',
  success_url: 'https://shop.example.com/orders/ORDER-1001',
  cancel_url: 'https://shop.example.com/cart',
});

// Send the customer to the checkout.
redirect(invoice.invoice_url);
```

## Create a payment

```ts
const payment = await turmite.createPayment({
  price_amount: 25,
  price_currency: 'usd',
  pay_currency: 'usdttrc20',
  order_id: 'ORDER-1001',
  ipn_callback_url: 'https://shop.example.com/turmite/ipn',
  idempotency_key: 'ORDER-1001',
});

// Show these on your own page, or send the customer to payment.payment_url.
console.log(payment.pay_address, payment.pay_amount);

const latest = await turmite.getPayment(payment.payment_id);
```

The server sends `payment_id` as a string in the `createPayment()` response and as a number in `getPayment()` and `listPayments()`. See [The payment object](/docs/merchant-api/#the-payment-object).

## Methods

| Method | Endpoint |
|---|---|
| `status()` | `GET /v1/status` |
| `currencies()` | `GET /v1/currencies` |
| `fullCurrencies()` | `GET /v1/full-currencies` |
| `merchantCoins()` | `GET /v1/merchant/coins` |
| `minAmount({ currency_from, currency_to, fiat_equivalent? })` | `GET /v1/min-amount` |
| `estimate({ amount, currency_from, currency_to })` | `GET /v1/estimate` |
| `createPayment(body)` | `POST /v1/payment` |
| `getPayment(paymentId)` | `GET /v1/payment/{payment_id}` |
| `listPayments(params)` | `GET /v1/payment` |
| `createInvoice(body)` | `POST /v1/invoice` |
| `createInvoicePayment(body)` | `POST /v1/invoice-payment` |
| `balance()` | `GET /v1/balance` |
| `auth(email, password)` | `POST /v1/auth` |
| `createPayout(body, options?)` | `POST /v1/payout` |
| `verifyPayout(batchId, verificationCode, options?)` | `POST /v1/payout/{batch_id}/verify` |
| `getPayout(batchId)` | `GET /v1/payout/{batch_id}` |
| `listPayouts(params, options?)` | `GET /v1/payout` |
| `validateAddress(body)` | `POST /v1/payout/validate-address` |
| `payoutFee({ currency, amount? })` | `GET /v1/payout/fee` |

The field names are the same as in the API. See [Merchant API](/docs/merchant-api/).

## Payouts

`auth()` signs in a dashboard user and keeps the token for `createPayout()`, `verifyPayout()` and `listPayouts()`. The token is valid for 5 minutes. To use another token, pass `{ token }` as the last argument of these methods. `createPayout()` and `verifyPayout()` need a user who is a project admin.

```ts
await turmite.auth('ops@example.com', password);
const batch = await turmite.createPayout({
  withdrawals: [
    { address: 'TQrZ9wBzhXnHdAdKwRBfdMH2RiTjH6yGtW', currency: 'usdttrc20', amount: '250.00', unique_external_id: 'refund-1042' },
  ],
});
await turmite.verifyPayout(batch.id, totpCode);
```

See [Payouts](/docs/payouts/).

## Verify webhooks

The SDK verifies both signatures. Pass the raw body, before JSON parsing. A string, a `Uint8Array` (for example a Node.js `Buffer`) or an `ArrayBuffer` works.

| Function | Checks | Returns |
|---|---|---|
| `verifyWebhook(rawBody, headers, secret, options?)` | `x-turmite-timestamp` and `x-turmite-signature` | The parsed body. Throws `WebhookVerificationError` when a check fails |
| `verifyIpnSignature(rawBody, signature, secret)` | The key-sorted HMAC-SHA512 signature, default header `x-turmite-sig` | `true` or `false`. It never throws for a bad signature or body |
| `isPaid(status)` | Nothing | `true` only for `finished` |

`verifyWebhook` refuses a timestamp that is more than 300 seconds from the local clock. Change it with `{ toleranceSeconds }`. `verifyIpnSignature` makes the key-sorted JSON from the raw body and keeps the text of each number, so the check also works for a number such as `1e-6`. `verifyWebhook` throws a `TypeError` when the body is not raw, for example a parsed object.

```ts
import express from 'express';
import { isPaid, verifyIpnSignature, verifyWebhook, type PaymentWebhook, type WebhookEvent } from '@turmite/sdk';

const app = express();

// Per-payment IPN (ipn_callback_url): timestamp signature, project IPN secret.
app.post('/turmite/ipn', express.raw({ type: 'application/json' }), async (req, res) => {
  let payment: PaymentWebhook;
  try {
    payment = await verifyWebhook<PaymentWebhook>(req.body, req.headers, process.env.TM_IPN_SECRET!);
  } catch {
    return res.sendStatus(400);
  }
  if (isPaid(payment.payment_status)) await markOrderPaid(payment.order_id, payment.payment_id);
  res.sendStatus(200);
});

// Project endpoint: envelope body, signed with the endpoint's own secret.
app.post('/turmite/webhook', express.raw({ type: 'application/json' }), async (req, res) => {
  const event = await verifyWebhook<WebhookEvent>(req.body, req.headers, process.env.TM_WEBHOOK_SECRET!).catch(() => null);
  if (!event) return res.sendStatus(400);
  if (event.event === 'payment.finished') await markOrderPaid(event.data.order_id, event.data.payment_id);
  res.sendStatus(200);
});

// Existing code that checks the compatible signature.
app.post('/legacy/ipn', express.raw({ type: 'application/json' }), async (req, res) => {
  const ok = await verifyIpnSignature(req.body, req.get('x-turmite-sig'), process.env.TM_IPN_SECRET!);
  if (!ok) return res.sendStatus(400);
  res.sendStatus(200);
});
```

Make `markOrderPaid` idempotent. See [Webhooks](/docs/webhooks/#handle-events-safely).

The package also exports the header names as `TIMESTAMP_HEADER`, `SIGNATURE_HEADER` and `IPN_SIGNATURE_HEADER`, and `canonicalJson()`, which returns the key-sorted JSON of a raw body.

## Errors

A failed request throws `TurmiteError`. It has `status` (the HTTP status, or `0` when no response came), `code`, `message` and `body`.

```ts
import { TurmiteError } from '@turmite/sdk';

try {
  await turmite.createPayment({ price_amount: 0, price_currency: 'usd', pay_currency: 'btc' });
} catch (err) {
  if (err instanceof TurmiteError) {
    console.error(err.status, err.code, err.message);
  }
}
```

`code` is the [API error code](/docs/errors/), for example `INVALID_REQUEST_PARAMS`. The SDK adds these codes of its own:

| Code | Cause |
|---|---|
| `NETWORK_ERROR` | The request failed before a response came |
| `TIMEOUT` | No response within `timeoutMs` |
| `TOKEN_REQUIRED` | `createPayout()`, `verifyPayout()` or `listPayouts()` was called before `auth()` and without a token |
| `HTTP_<status>` | An error response without a JSON `code` |

`WebhookVerificationError` has a `code`: `missing_header`, `invalid_timestamp`, `timestamp_out_of_range`, `invalid_signature` or `invalid_body`.

## Types

The package exports a type for each request and response, for example `Payment`, `PaymentWebhook`, `PaymentStatus`, `Invoice`, `Payout`, `PayoutStatus`, `PayoutBatch` and `WebhookEvent`. It also exports the option types `TurmiteOptions`, `TokenOptions`, `FetchLike`, `VerifyWebhookOptions`, `HeadersLike`, `RawBody` and `WebhookErrorCode`. The types follow the [API reference](/docs/api-reference/).
