---
title: Checkout
description: The invoice page and the payment page that customers see, how they work without JavaScript, and how to brand them.
sidebarTitle: Checkout
---

Turmite serves the pages that customers use to pay. Every page works with JavaScript off. All motion on the pages is CSS.

## Pages

| Path | Page |
|---|---|
| `/i/{invoice_token}` | Invoice. Shows the amount and the coins, grouped by network |
| `/pay/{payment_token}` | Payment. Shows the amount, the network, the address, the memo, the QR code and the countdown |
| `/pay/{payment_token}/status` | Status. Loaded in a frame on the payment page |
| `/x/{payment_token}` | Pay with another coin: the coin list. See [Pay with any coin](/docs/exchange/) |
| `/x/{swap_token}` | The swap: the exchange's deposit address and amount |
| `/x/{swap_token}/status` | Swap status. Loaded in a frame on the swap page |

The tokens are random. They are not the numeric `payment_id`, so a customer cannot guess the link of another payment.

## How the customer pays

1. Your shop creates an invoice with `POST /v1/invoice` and sends the customer to `invoice_url`.
2. The customer selects a coin. Each coin is a form button. An invoice with a `pay_currency` skips this step.
3. Turmite creates the payment and redirects the customer to `/pay/{payment_token}`. When the customer selects the same coin again, Turmite uses the open payment.
4. The customer sends the amount to the address. The QR code is inline SVG. **Open in wallet** opens a payment link in the customer's wallet app.
5. The status frame shows the progress until the payment is final.
6. When the payment is paid, the frame shows a "Return to {store}" button. When the payment ends while the page is open, the frame also shows **Done**. It loads the page again without the address and the QR code.

While the invoice is open and the payment is not final, the payment page has the link **Choose another coin**.

If you create the payment with `POST /v1/payment`, your shop shows the address and the amount on its own page. See [Merchant API](/docs/merchant-api/#payments-or-invoices).

## Without JavaScript

The pages use HTML forms and CSS only.

- The coin list is a form. Each coin button sends a `POST`. Turmite answers with a `303` redirect to the payment page.
- The status frame reloads itself with `<meta http-equiv="refresh">` every 4 seconds. It stops when the payment is `finished`, `expired`, `failed` or `refunded`.
- Each reload adds `?seen=` with the state that the frame showed. The animation of a new state plays only once.
- Looping animations take their position from the server clock, so a reload does not restart them.
- The countdown is a CSS animation. Its duration is the number of seconds until the payment window closes. Then it shows "Expired" and the page dims the address.

The customer does not need to reload the page.

## Status on the page

| Title on the page | Payment status | The frame shows |
|---|---|---|
| Waiting for payment | `waiting` | "Send {amount} {coin} to the address below." The countdown |
| Payment received | `confirming` | The confirmations, for example `1 of 3`, and an estimate of the time left |
| Partly paid | `partially_paid` | The amount received and the rest. "Send {rest} {coin} more to the same address." The countdown |
| Payment confirmed | `confirmed` | "Almost done." |
| Paid | `finished` | A check mark that draws itself, in green. The "Return to {store}" button |
| Expired | `expired` | A cross. "The payment window closed. Do not send funds to this address." |
| Payment failed | `failed` | A cross, in red. "This payment cannot be completed. Contact {store}." |
| Refunded | `refunded` | "{store} sent this payment back." |

The status frame shows a 7 x 7 grid with the Turmite mark. The confirmations fill the grid. The number of confirmations comes from the network settings. See [Networks and nodes](/docs/nodes/#confirmations).

Green and red are the only status colours. Green means paid. Red means failed.

## Reduced motion

The pages respect `prefers-reduced-motion`. When the customer's system asks for reduced motion, nothing moves. The countdown still counts down.

## Return to the store

The "Return to {store}" button opens:

| Payment status | Target |
|---|---|
| `finished` | The `success_url` of the payment or the invoice. Else the project's **Return URL** |
| `expired`, `failed` | The `cancel_url` of the payment or the invoice. Else the project's **Return URL** |
| `refunded` | The project's **Return URL** |

Each payment keeps the project's **Return URL** of the time when the payment was created. Without a target, the page shows no button. When the invoice is still open, an expired payment also shows **Start again**.

When an invoice or a payment has a `cancel_url`, the page shows the link "Cancel and return to {store}".

The button is for the customer only. Mark the order as paid from a webhook. Do not mark it as paid when the customer opens the return page. See [Webhooks](/docs/webhooks/).

### Allowed return hosts

Set the hosts in **Settings → Checkout → Allowed return hosts**, one host per line, at most 50.

- When the list is empty, Turmite accepts a `success_url` and a `cancel_url` on any host.
- When the list has hosts, the merchant API answers `400` with `INVALID_REQUEST_PARAMS` for a `success_url` or `cancel_url` on another host. The **Return URL** must also use one of the hosts.
- The host must match exactly. Add `www.shop.example.com` and `shop.example.com` separately if you use both.

All these URLs must use `https://` on a mainnet install. A testnet install also accepts `http://`.

## Branding

Each project has its own branding. Set it in **Settings → Checkout**. You need the `project_admin` role. Saving needs no two-factor code. The preview on the right shows the checkout with sample data and your saved settings.

| Setting | Effect |
|---|---|
| **Image file** or **Or an image URL** | The logo at the top of the page. PNG, JPEG or WebP, at most 2 MB. SVG is refused. Turmite scales the logo to fit 512 x 512 pixels and stores it as PNG |
| **Theme** | **Auto**, **Light** or **Dark**. Auto follows the customer's system |
| **Accent colour** | Used for buttons, progress and the countdown. **Use black and white** keeps the default |
| **Message on the checkout** | Short text on the page, for example delivery times. At most 280 characters |
| **Support email**, **Support page** | Links at the foot of the page |
| **Return URL** | Default target of the "Return to {store}" button |
| **Allowed return hosts** | See above |

The project name in **Settings → General** is the store name on the checkout. Without a logo, the page shows the first letter of the name.

Turmite downloads a logo URL once. The customer's browser loads the logo from Turmite, not from your site. The source image can have at most 8192 pixels on each side.

The time limits are in **Settings → General**:

| Setting | Default | Range | Effect |
|---|---|---|---|
| **Payment window (minutes)** | 60 | 5 to 10080 | Time to pay once a coin is chosen. The countdown uses it |
| **Invoice window (minutes)** | 1440 | 5 to 43200 | How long an invoice link stays open |

## Checkout domains

A project can show its checkout on its own host name, for example `pay.shop-two.com`. On that host, Turmite serves only `/pay/`, `/i/`, `/x/`, `/v1/`, `/assets/`, `/branding/`, `/healthz` and `/favicon.svg`. The pages of other projects answer `404`. See [Projects and checkout domains](/docs/projects/#give-a-project-a-checkout-domain).

## Security

The checkout pages send this content security policy:

```text
default-src 'none'; style-src 'self'; style-src-attr 'unsafe-inline'; img-src 'self' data:; font-src 'self'; frame-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'
```

The status frames send the same policy without `frame-src`, and with `frame-ancestors 'self'`.

- The pages run no scripts.
- `frame-ancestors 'none'` and `X-Frame-Options: DENY` stop other sites from showing the checkout in a frame. Link or redirect to the checkout. Do not put it in an iframe on your shop.
- Forms only send to Turmite.
- The status frame comes from the same host as the payment page.
- The pages send `Cache-Control: no-store`.
- Each IP address can load 300 checkout pages per minute.
