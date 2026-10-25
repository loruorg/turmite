# @turmite/sdk

TypeScript client for the [Turmite](https://turmite.com) merchant API, with webhook verification.
It has no runtime dependencies. It runs on Node.js 20+, Deno, Bun and Cloudflare Workers.

## Install

```sh
npm install @turmite/sdk
```

## Use

`baseUrl` is the site URL of your Turmite server. It can have a path.

```ts
import { Turmite } from '@turmite/sdk';

const turmite = new Turmite({ baseUrl: 'https://pay.example.com', apiKey: 'tm_…' });
```

Send the customer to a hosted invoice. The customer selects the coin there.

```ts
const invoice = await turmite.createInvoice({
  price_amount: 49.9,
  price_currency: 'usd',
  order_id: '1042',
  ipn_callback_url: 'https://shop.example.com/turmite/webhook',
  success_url: 'https://shop.example.com/orders/1042',
  cancel_url: 'https://shop.example.com/cart',
});
redirect(invoice.invoice_url);
```

Make a payment for your own checkout page, then read its status.

```ts
const payment = await turmite.createPayment({
  price_amount: 49.9,
  price_currency: 'usd',
  pay_currency: 'usdttrc20',
  order_id: '1042',
  ipn_callback_url: 'https://shop.example.com/turmite/webhook',
});
show(payment.pay_address, payment.pay_amount);

const latest = await turmite.getPayment(payment.payment_id);
```

`createPayment()` returns `payment_id` and `purchase_id` as strings. `getPayment()`, `listPayments()` and
webhooks return `payment_id` as a number. This is the same as in the common hosted-gateway API.

Send a payout. A dashboard user signs in, and the TOTP code of that user approves the batch.

```ts
await turmite.auth('ops@example.com', password);
const batch = await turmite.createPayout({
  withdrawals: [{ address: 'TQ5…', currency: 'usdttrc20', amount: '120.00', unique_external_id: 'refund-1042' }],
});
await turmite.verifyPayout(batch.id, totpCode);
```

A failed request throws `TurmiteError` with `status`, `code` and `message`.

## Webhooks

Each delivery has these signatures:

- `x-turmite-timestamp` and `x-turmite-signature`: `verifyWebhook()` checks them and the time (5 minutes by default).
  Then it returns the parsed body.
- `x-turmite-sig` (the name is a project setting): HMAC-SHA512 of the key-sorted body. Existing integrations of
  the same API shape check this header. `verifyIpnSignature()` checks it.

Give the raw body to both functions. A parsed and serialized body can have different number text.
Ship an order only when `isPaid(payment.payment_status)` is true.

Express:

```ts
import express from 'express';
import { isPaid, verifyWebhook, type PaymentWebhook } from '@turmite/sdk';

app.post('/turmite/webhook', express.raw({ type: 'application/json' }), async (req, res) => {
  let payment: PaymentWebhook;
  try {
    payment = await verifyWebhook<PaymentWebhook>(req.body, req.headers, process.env.IPN_SECRET!);
  } catch {
    return res.sendStatus(400);
  }
  if (isPaid(payment.payment_status)) await markPaid(payment.order_id);
  res.sendStatus(200);
});
```

Hono:

```ts
app.post('/turmite/webhook', async (c) => {
  const raw = await c.req.text();
  const payment = await verifyWebhook<PaymentWebhook>(raw, c.req.raw.headers, c.env.IPN_SECRET).catch(() => null);
  if (!payment) return c.text('invalid signature', 400);
  if (isPaid(payment.payment_status)) await markPaid(payment.order_id);
  return c.text('ok');
});
```

Next.js route handler (`app/turmite/webhook/route.ts`):

```ts
import { isPaid, verifyIpnSignature, type PaymentWebhook } from '@turmite/sdk';

export async function POST(req: Request) {
  const raw = await req.text();
  if (!(await verifyIpnSignature(raw, req.headers.get('x-turmite-sig'), process.env.IPN_SECRET!))) {
    return new Response('invalid signature', { status: 400 });
  }
  const payment: PaymentWebhook = JSON.parse(raw);
  if (isPaid(payment.payment_status)) await markPaid(payment.order_id);
  return new Response('ok');
}
```

The secret is the project IPN secret for an `ipn_callback_url`. A webhook endpoint that you add in the dashboard
has its own secret. Its body is an envelope: `{ event, delivery_id, created_at, data }`.
Use `verifyWebhook<WebhookEvent>()` for it.

## License

MIT
