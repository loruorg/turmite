---
title: Merchant API
description: Use the merchant API to create payments and invoices, read their status and send payouts.
sidebarTitle: Merchant API guide
---

Your shop's server calls the merchant API to create payments and invoices and to read their status. The API keeps the paths, fields, statuses and IPN signature of the common hosted-gateway API. The [API reference](/docs/api-reference/) lists every field and schema.

## Base URL

The base URL is your site URL (`TURMITE_SITE_URL`). Every path starts with `/v1/`.

| Site URL | Example request |
|---|---|
| `https://pay.example.com` | `https://pay.example.com/v1/payment` |
| `https://example.com/pay` | `https://example.com/pay/v1/payment` |

A project's checkout domain also serves the merchant API. A shop can use its own checkout domain as the base URL. See [Projects and checkout domains](/docs/projects/).

## Authentication

Send the project API key in the `x-api-key` header. Keys start with `tm_`. Each key belongs to one project, and the API only shows data of that project.

1. Open the dashboard and select the project.
2. Open **API keys**.
3. Under **Create a key**, enter a label and a fresh two-factor code. Then click **Create key**.
4. Copy the key. The dashboard shows it one time.

You need the `project_admin` role to create a key. A `developer` can revoke a key. Turmite stores only a SHA-256 hash of each key.

Keep the key on your server. Do not put it in browser code or in a mobile app.

```bash
curl https://pay.example.com/v1/status
curl -H "x-api-key: $TM_API_KEY" https://pay.example.com/v1/merchant/coins
```

The payout endpoints also need a short-lived token from `POST /v1/auth`. See [Payouts](#payouts).

## Requests and responses

- Send JSON bodies with `Content-Type: application/json`. Turmite ignores fields that it does not know.
- Tickers (`pay_currency`, `currency`) are lowercase, for example `btc` or `usdttrc20`. See [Tickers and networks](/docs/networks/).
- Request amounts can be a JSON number or a decimal string, for example `25` or `"25.00"`.
- `POST /v1/payment`, `POST /v1/invoice`, `POST /v1/invoice-payment` and `POST /v1/payout` answer `201` with the new object. A retry that returns an existing object also gets `201`.
- Payment amounts in responses are JSON numbers. A whole amount is an integer: `25`, not `25.0`. An amount below 0.00001 can be in exponent form, for example `1e-6`. Invoice amounts, payout amounts and `estimated_amount` are decimal strings.
- Timestamps are ISO 8601 in UTC, with milliseconds.
- Each client IP address can send 600 requests per minute. Above that, the API returns `429`.
- An error has this shape. See [Errors](/docs/errors/).

```json
{"status": false, "statusCode": 400, "code": "INVALID_REQUEST_PARAMS", "message": "Currency `btc` is not enabled for this project. Add a Bitcoin wallet in the dashboard."}
```

### Price currencies and rates

`price_currency` is a currency code that the rate source knows, for example `usd`, `eur` or `btc`. Responses write it in lowercase.

Turmite gets rates from CoinGecko. When CoinGecko fails, it uses Coinbase. It keeps each rate for 60 seconds. USDT and USDC count as 1 USD. When `price_currency` is the ticker or the symbol of `pay_currency`, for example `usdt` for `usdttrc20`, Turmite does not convert the price.

When no source gives a rate, the request gets `503` with the code `UNAVAILABLE`. Try again after a minute.

## Endpoints

| Method | Path | Auth | Use |
|---|---|---|---|
| GET | `/v1/status` | none | Health of the API |
| GET | `/v1/currencies` | key optional | Enabled tickers |
| GET | `/v1/full-currencies` | key optional | Details per ticker |
| GET | `/v1/merchant/coins` | key | Tickers enabled for the project |
| GET | `/v1/min-amount` | none | Smallest payment for a ticker |
| GET | `/v1/estimate` | none | Price conversion |
| POST | `/v1/payment` | key | Create a payment |
| GET | `/v1/payment/{payment_id}` | key | Read one payment |
| GET | `/v1/payment` | key | List payments |
| POST | `/v1/invoice` | key | Create an invoice with a checkout link |
| POST | `/v1/invoice-payment` | key | Create a payment for an invoice |
| GET | `/v1/balance` | key | Credited and pending amounts per ticker |
| POST | `/v1/auth` | none | Get a token for payouts |
| POST | `/v1/payout` | key and token | Create a payout batch |
| POST | `/v1/payout/{batch_id}/verify` | key and token | Confirm a batch with a two-factor code |
| GET | `/v1/payout/{batch_id}` | key | Read one batch |
| GET | `/v1/payout` | key and token | List payouts |
| POST | `/v1/payout/validate-address` | key | Check a destination address |
| GET | `/v1/payout/fee` | key | Estimate the network fee |

"Key optional" endpoints answer for the key's project when you send a key, and for the whole install when you do not. A wrong key gets `401`.

## Status and currencies

`GET /v1/status` returns `{"message":"OK"}` when the API is up. For load balancer checks, use `/healthz`. It returns `ok`.

`GET /v1/currencies` returns the tickers that the key's project accepts. A project accepts a ticker when the ticker and its network are enabled and the project has a wallet on that network. Without a key, the list has the tickers that at least one project accepts.

```bash
curl -H "x-api-key: $TM_API_KEY" https://pay.example.com/v1/currencies
```

```json
{"currencies": ["btc", "xmr", "eth", "usdc", "usdterc20", "usdcbase", "usdttrc20", "sol"]}
```

`GET /v1/full-currencies` returns every enabled ticker of the install, with details.

```json
{
  "currencies": [
    {
      "id": 7,
      "code": "USDTTRC20",
      "name": "Tether USD",
      "enable": true,
      "wallet_regex": null,
      "priority": 7,
      "extra_id_exists": false,
      "extra_id_regex": null,
      "logo_url": null,
      "track": false,
      "cg_id": "tether",
      "is_maxlimit": false,
      "network": "tron",
      "smart_contract": "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t",
      "network_precision": "6",
      "ticker": "usdttrc20"
    }
  ]
}
```

`ticker` is the value to send as `pay_currency`. `code` is the ticker in upper case. `enable` is true when the project accepts the ticker. `network_precision` is the number of decimals, as a string. `smart_contract` is `null` for a native coin. `wallet_regex`, `extra_id_regex` and `logo_url` are always `null`.

`GET /v1/merchant/coins` returns the tickers that the project accepts.

```json
{"selectedCurrencies": ["btc", "usdcbase", "usdttrc20"]}
```

## Estimates

`GET /v1/min-amount` returns the smallest payment for a ticker.

| Parameter | Required | Description |
|---|---|---|
| `currency_from` | yes | The ticker, for example `btc` |
| `currency_to` | no | Copied to the response. The default is `currency_from` |
| `fiat_equivalent` | no | A fiat code. The response then has `fiat_equivalent`, the minimum in that currency, when a rate is available |

```bash
curl "https://pay.example.com/v1/min-amount?currency_from=btc&fiat_equivalent=usd"
```

```json
{"currency_from": "btc", "currency_to": "btc", "min_amount": 1e-6, "fiat_equivalent": 0.07}
```

When the operator set no minimum for the ticker, `min_amount` is a default that depends on the decimals of the ticker. `POST /v1/payment` refuses an amount below `min_amount` with `400` and the code `INVALID_REQUEST_PARAMS`.

`GET /v1/estimate` converts a price into a ticker amount at the current rate. Turmite rounds the amount up to the decimals of the ticker.

| Parameter | Required | Description |
|---|---|---|
| `amount` | yes | The price |
| `currency_from` | yes | The price currency, for example `usd` |
| `currency_to` | yes | The ticker, for example `btc` |

```bash
curl "https://pay.example.com/v1/estimate?amount=25&currency_from=usd&currency_to=btc"
```

```json
{"currency_from": "usd", "amount_from": 25, "currency_to": "btc", "estimated_amount": "0.00041235"}
```

## Payments or invoices

Turmite has two ways to take a payment.

| Use | Create | The customer sees |
|---|---|---|
| Your shop shows the address and the amount on its own page | `POST /v1/payment` | Your page |
| Turmite shows the checkout | `POST /v1/invoice` | The invoice page at `invoice_url`, then the payment page |

The checkout pages work with JavaScript off. See [Checkout](/docs/checkout/).

## Create a payment

`POST /v1/payment` creates a payment with its own deposit address.

| Field | Required | Description |
|---|---|---|
| `price_amount` | yes | Price in `price_currency`. It must be more than 0 |
| `price_currency` | yes | Currency of the price, for example `usd` |
| `pay_currency` | yes | Ticker the customer pays with, for example `btc`. The project must accept it |
| `pay_amount` | no | A fixed amount in `pay_currency`. Without it, Turmite converts `price_amount` at the current rate |
| `order_id` | no | Your order number, up to 255 characters. Webhooks include it |
| `order_description` | no | Text for your records, up to 1000 characters |
| `ipn_callback_url` | no | URL that receives a webhook at each status change |
| `success_url` | no | Page for the "Return to store" button after payment |
| `cancel_url` | no | Page for the return button after the payment expires |
| `purchase_id` | no | Your purchase reference, up to 255 characters. Without it, Turmite makes a 10-digit one |
| `idempotency_key` | no | Up to 255 characters. A retry with the same key returns the first payment. You can send it in the `Idempotency-Key` header instead |

The three URLs must be `https` URLs without a user name or password. A testnet install also accepts `http`. If the project has **Allowed return hosts** under **Settings** → **Checkout**, `success_url` and `cancel_url` must use one of these hosts.

Send `idempotency_key` when your shop can retry the request, for example after a timeout. Use one key per order. A request with a known key returns the first payment, even when other fields are different.

```bash
curl -X POST https://pay.example.com/v1/payment \
  -H "x-api-key: $TM_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "price_amount": 25,
    "price_currency": "usd",
    "pay_currency": "btc",
    "order_id": "ORDER-1001",
    "order_description": "Order 1001",
    "ipn_callback_url": "https://shop.example.com/turmite/ipn",
    "idempotency_key": "ORDER-1001"
  }'
```

The response is `201` with a [payment object](#the-payment-object) in the status `waiting`. Show `pay_address` and `pay_amount` to the customer on your own page, or send the customer to `payment_url`, the hosted payment page. Save `payment_id` with the order.

```json
{
  "payment_id": "5524759814",
  "invoice_id": null,
  "payment_status": "waiting",
  "pay_address": "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
  "payin_extra_id": null,
  "price_amount": 25,
  "price_currency": "usd",
  "pay_amount": 0.00041235,
  "actually_paid": 0,
  "pay_currency": "btc",
  "order_id": "ORDER-1001",
  "order_description": "Order 1001",
  "purchase_id": "4709931082",
  "outcome_amount": null,
  "outcome_currency": "btc",
  "network": "btc",
  "network_precision": 8,
  "smart_contract": null,
  "payin_hash": null,
  "payout_hash": null,
  "payment_url": "https://pay.example.com/pay/Q7mX2pK9vT4nB8cR1sW6yZ3h",
  "created_at": "2026-10-23T12:00:00.000Z",
  "updated_at": "2026-10-23T12:00:00.000Z",
  "expiration_estimate_date": "2026-10-23T13:00:00.000Z",
  "parent_payment_id": null,
  "type": "crypto2crypto",
  "time_limit": null,
  "burning_percent": null,
  "payment_extra_ids": null,
  "amount_received": 0.00041235,
  "actually_paid_at_fiat": 0,
  "fee": {"currency": "btc", "depositFee": 0, "withdrawalFee": 0, "serviceFee": 0}
}
```

In this response, `payment_id` and `purchase_id` are strings. See [The payment object](#the-payment-object) for the types in the other responses.

A payment expires after the project's payment window, 60 minutes by default. Change it in **Settings** under **Payment window (minutes)**, from 5 to 10080.

When the network has no free deposit address, or no node is connected for it, the request gets `503` with `Retry-After: 30`. Send it again after 30 seconds.

## Create an invoice

`POST /v1/invoice` creates an invoice and returns `invoice_url`. Send the customer to that URL. The customer selects a coin, and Turmite creates the payment.

| Field | Required | Description |
|---|---|---|
| `price_amount` | yes | Price in `price_currency` |
| `price_currency` | yes | Currency of the price |
| `pay_currency` | no | Ticker to use. Set it to skip the coin selection on the invoice page |
| `order_id` | no | Your order number |
| `order_description` | no | Text for your records |
| `ipn_callback_url` | no | URL that receives the webhooks of the invoice's payments |
| `success_url` | no | Page for the "Return to store" button |
| `cancel_url` | no | Page for the return button after the payment expires |

The limits and URL rules are the same as for `POST /v1/payment`. The payment that the customer starts copies `order_id`, `order_description`, `ipn_callback_url`, `success_url` and `cancel_url` from the invoice.

```bash
curl -X POST https://pay.example.com/v1/invoice \
  -H "x-api-key: $TM_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "price_amount": 25,
    "price_currency": "usd",
    "order_id": "ORDER-1001",
    "order_description": "Order 1001",
    "ipn_callback_url": "https://shop.example.com/turmite/ipn",
    "success_url": "https://shop.example.com/orders/1001",
    "cancel_url": "https://shop.example.com/cart"
  }'
```

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
  "success_url": "https://shop.example.com/orders/1001",
  "cancel_url": "https://shop.example.com/cart",
  "created_at": "2026-10-23T12:00:00.000Z",
  "updated_at": "2026-10-23T12:00:00.000Z",
  "expiration_estimate_date": "2026-10-24T12:00:00.000Z"
}
```

`id` is numeric text. `price_amount` is a decimal string. An invoice does not change, so `updated_at` is always `created_at`. An invoice stays open for the project's invoice window, 1440 minutes (24 hours) by default. Change it in **Settings** under **Invoice window (minutes)**, from 5 to 43200.

If the project has a checkout domain, `invoice_url` uses it.

### Create a payment for an invoice

`POST /v1/invoice-payment` creates the payment for an invoice from your own coin picker. Send the invoice `id` as `iid` and the ticker as `pay_currency`. `purchase_id` is optional. The response is a payment object with `invoice_id` set. The payment gets the price, the order and the URLs of the invoice.

If the invoice already has an open payment in that ticker, the response is that payment. An expired invoice gets `400`.

```bash
curl -X POST https://pay.example.com/v1/invoice-payment \
  -H "x-api-key: $TM_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"iid": "4522625843", "pay_currency": "usdttrc20"}'
```

## Read payments

`GET /v1/payment/{payment_id}` returns one payment. A payment of another project gets `404`.

```bash
curl -H "x-api-key: $TM_API_KEY" https://pay.example.com/v1/payment/5524759814
```

`GET /v1/payment` returns a page of payments.

| Parameter | Description |
|---|---|
| `limit` | Payments per page, from 1 to 500. The default is 10 |
| `page` | Page number. The first page is 0 |
| `sortBy` | `created_at` (default), `updated_at`, `payment_id`, `payment_status`, `pay_address`, `price_amount`, `price_currency`, `pay_amount`, `actually_paid`, `pay_currency`, `outcome_amount`, `outcome_currency`, `order_id`, `order_description` or `purchase_id` |
| `orderBy` | `asc` or `desc`. The default is `desc` |
| `dateFrom`, `dateTo` | Range of `created_at`. A date (`2026-10-01`) or a date-time (`2026-10-31T23:59:59Z`). A date in `dateTo` includes the full day |
| `payment_status` | Only payments with this status |
| `order_id` | Only payments with this order ID |
| `invoiceId` | Only payments of this invoice |

```bash
curl -H "x-api-key: $TM_API_KEY" \
  "https://pay.example.com/v1/payment?limit=10&page=0&sortBy=created_at&orderBy=desc"
```

```json
{"data": [{"payment_id": 5524759814, "payment_status": "finished"}], "limit": 10, "page": 0, "pagesCount": 1, "total": 1}
```

The example shows two fields of each payment. Each item in `data` is a full payment object.

Use webhooks to learn about status changes. Use these endpoints to check a status again or to reconcile.

## Balance

`GET /v1/balance` returns two amounts for each ticker that has payments in the project.

| Field | Value |
|---|---|
| `amount` | The sum of `outcome_amount` of the project's payments, refunded payments excluded |
| `pendingAmount` | The amount still due on open payments (`waiting`, `confirming`, `partially_paid`, `confirmed`): `pay_amount` minus the credited amount |

```json
{"btc": {"amount": 0.0153, "pendingAmount": 0.0012}, "usdttrc20": {"amount": 1250.5, "pendingAmount": 0}}
```

The balance is a total of payments. It is not the balance of a wallet. Payouts do not decrease it. A payment that you mark as refunded leaves it.

## Payouts

Payouts send funds from the project's hot wallet. Read [Payouts](/docs/payouts/) for the setup, the limits and the approval rules.

1. `POST /v1/auth` with the email address and password of a dashboard user returns a token. The token is valid for 5 minutes.
2. `POST /v1/payout` with the API key and the token creates a batch. The user must be a project admin. Each address must be in the project's address book. New payouts have the status `pending-otp-verification`.
3. `POST /v1/payout/{batch_id}/verify` with the user's current two-factor code releases the batch. The project limits then set each payout to `pending` or `pending-approval`.

The full example is in [Create a payout with the API](/docs/payouts/#create-a-payout-with-the-api). `POST /v1/auth` accepts 10 attempts per minute for each IP address and for each email address.

## The payment object

| Field | Description |
|---|---|
| `payment_id` | ID of the payment |
| `invoice_id` | ID of the invoice as a number, or `null` |
| `payment_status` | Status. See [Payment statuses](/docs/statuses/) |
| `pay_address` | Deposit address for this payment |
| `payin_extra_id` | Always `null`. No supported network needs a deposit memo |
| `price_amount` | Price in `price_currency` |
| `price_currency` | Currency of the price |
| `pay_amount` | Amount the customer must send, in `pay_currency` |
| `actually_paid` | Amount of all deposits seen so far, confirmed or not |
| `pay_currency` | Ticker the customer pays with |
| `order_id` | Your order number |
| `order_description` | Your description |
| `purchase_id` | Your purchase reference, or a 10-digit number that Turmite makes |
| `outcome_amount` | Confirmed amount credited to the project, or `null` before the first credit. Sweep fees are not subtracted |
| `outcome_currency` | Same as `pay_currency` |
| `network` | Network code: `btc`, `xmr`, `eth`, `base`, `polygon`, `bsc`, `tron`, `sol` or `ton` |
| `network_precision` | Decimals of `pay_currency` |
| `smart_contract` | Token contract, or `null` for a native coin |
| `payin_hash` | Hash of the latest deposit transaction, or `null` |
| `payout_hash` | Hash of the latest sweep from the deposit address to your wallet, or `null`. Always `null` on Bitcoin and Monero |
| `payment_url` | Hosted payment page, `<site URL or checkout domain>/pay/<token>` |
| `created_at` | Time of creation |
| `updated_at` | Time of the last change |
| `expiration_estimate_date` | End of the payment window |
| `amount_received` | Amount the project gets in `outcome_currency`: `outcome_amount` after the first credit, `pay_amount` before it |
| `actually_paid_at_fiat` | `actually_paid` in `price_currency` at the rate of the payment. For a fixed `pay_amount`, the rate is `price_amount` / `pay_amount` |
| `fee` | `{currency, depositFee, withdrawalFee, serviceFee}`. Turmite takes no fee: `depositFee` and `serviceFee` are `0`. `withdrawalFee` is the network fee of the sweeps to your wallet |
| `type` | Always `crypto2crypto` |
| `parent_payment_id`, `time_limit`, `burning_percent`, `payment_extra_ids` | Always `null` |

The types of two IDs depend on the response. Typed clients of the common hosted-gateway API expect these types.

| Response | `payment_id` | `purchase_id` |
|---|---|---|
| `POST /v1/payment`, `POST /v1/invoice-payment` | String | String |
| `GET /v1/payment/{payment_id}`, `GET /v1/payment` | Number | Number when it is a plain integer up to 2^53 - 1, else string |
| Webhooks | Number | String |

The payment object keeps the field names of the common hosted-gateway API. `network_precision`, `smart_contract`, `payin_hash`, `payout_hash` and `payment_url` are extra fields.

The checkout page uses a random token in its URL. The numeric `payment_id` never appears in a checkout link.

## Statuses in short

Ship the order when the status is `finished`. The full list and the order of statuses are in [Payment statuses](/docs/statuses/). Webhooks tell you about each change. See [Webhooks](/docs/webhooks/).
