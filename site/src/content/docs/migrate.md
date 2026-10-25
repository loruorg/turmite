---
title: Migrate from a hosted gateway
description: Move an integration of the common hosted-gateway API to Turmite by changing the base URL, the API key and the IPN secret.
sidebarTitle: Migrate
---

The Turmite merchant API uses the same paths, fields, statuses and IPN signature as the common hosted-gateway API. Your existing integration keeps its code. You change the base URL, the API key and the IPN secret.

## What stays the same

| Part | Detail |
|---|---|
| Paths | `/v1/payment`, `/v1/invoice`, `/v1/invoice-payment`, `/v1/currencies`, `/v1/estimate`, `/v1/min-amount`, `/v1/payout` and the others in [Merchant API](/docs/merchant-api/) |
| Authentication | The `x-api-key` header |
| Request fields | `price_amount`, `price_currency`, `pay_currency`, `order_id`, `order_description`, `ipn_callback_url`, `success_url`, `cancel_url` |
| Payment object | `payment_id`, `payment_status`, `pay_address`, `pay_amount`, `actually_paid`, `outcome_amount` and the other fields |
| Statuses | `waiting`, `confirming`, `confirmed`, `partially_paid`, `finished`, `refunded`, `expired`. `sending` and `failed` are in the API, but Turmite does not set them |
| Tickers | The same codes, for example `btc`, `usdttrc20`, `usdcsol`. See [Tickers and networks](/docs/networks/) |
| IPN body | The flat payment object, sent to `ipn_callback_url` |
| IPN signature | Hex HMAC-SHA512 of the JSON body with keys sorted, signed with the IPN secret |
| Errors | `{"status": false, "statusCode": 400, "code": "INVALID_REQUEST_PARAMS", "message": "..."}` |
| Payouts | `POST /v1/auth` for a JWT, then `POST /v1/payout`, then `POST /v1/payout/{batch_id}/verify` |

## What changes

| Setting | Before | After |
|---|---|---|
| Base URL | The gateway's API URL | Your site URL, for example `https://pay.example.com` |
| API key | The gateway's key | A key from your dashboard. It starts with `tm_` |
| IPN secret | The gateway's IPN secret | The IPN secret of your Turmite project |
| Signature header | The gateway's header name | `x-turmite-sig` by default. You can change it |
| Payout login | Your gateway account | The email and password of a Turmite dashboard user who is a project admin |
| Payout verification code | A code from the gateway | The current two-factor (TOTP) code of that dashboard user |

The paths start with `/v1`. If your configuration stores the base URL with `/v1` at the end, set it to `https://pay.example.com/v1`. If it stores the host only, set it to `https://pay.example.com`.

### Keep your signature header

Your webhook handler reads the signature from one header name. You have two options:

1. In the dashboard, open **Settings**. On the **General** tab, set **IPN signature header** to the header name that your code reads. Your handler needs no change.
2. Change your handler to read `x-turmite-sig`.

Turmite also sends `x-turmite-timestamp` and `x-turmite-signature`. That signature covers the timestamp and the raw body, so a receiver can reject replays. You can move to it later. See [Webhooks](/docs/webhooks/).

## Before you switch

Turmite is self-hosted. Before your shop can use it, you set up the parts that a hosted gateway runs for you.

1. Install a server. See [Install on a server](/docs/install/).
2. Add RPC nodes for each network that you accept. See [Networks and nodes](/docs/nodes/).
3. Set a wallet for each network. Funds go to these wallets. See [Wallets per chain](/docs/wallets/).
4. Enable the tickers that your shop sends as `pay_currency`.
5. For payouts, add and fund a hot wallet. See [Payouts](/docs/payouts/).

## Checklist

- [ ] A testnet install works and a test payment reached `finished`.
- [ ] Every `pay_currency` that your shop sends is in `GET /v1/currencies`.
- [ ] Your integration uses only the endpoints and fields in [Merchant API](/docs/merchant-api/).
- [ ] The base URL points at your server and keeps the `/v1` rule above.
- [ ] The API key is a new `tm_` key, stored on the shop server only.
- [ ] The IPN secret is the Turmite project secret.
- [ ] The signature header name matches your handler, or your handler reads `x-turmite-sig`.
- [ ] `ipn_callback_url` is an `https://` URL that answers directly. Mainnet refuses private addresses and does not follow redirects.
- [ ] Your handler ships only on `finished` and is idempotent.
- [ ] If you use payouts: a project admin with two-factor authentication exists for the API, the hot wallet is funded, the address book has your destinations and the limits are set.
- [ ] You did one small real payment on each network on the mainnet install.

## Differences to know

- You run the server, the database and the nodes. If the server is down, customers cannot open the checkout. When the worker starts again, it finds the deposits made during the downtime.
- Funds go to your own wallets. On Bitcoin and Monero they arrive directly. On other chains a deposit address forwards them to your cold wallet.
- Turmite does not convert. The customer's coin goes to your wallet, and `outcome_currency` is always `pay_currency`.
- `GET /v1/balance` is a total of credited payments. It is not a balance that Turmite holds for you.
- Turmite supports only the networks and tickers in [Tickers and networks](/docs/networks/). A ticker that is not enabled returns `400`.
- `payin_extra_id` is always `null`. No supported network needs a deposit memo.
- `POST /v1/payment` returns `payment_id` as a string. `GET /v1/payment` and webhooks send it as a number. Store it as a string.
- An endpoint that Turmite does not have returns `404` with the code `NOT_FOUND`. Turmite ignores request fields that it does not know.
- Payouts work from hot wallets on the EVM networks, Solana, Tron and TON. Bitcoin and Monero have no hot wallet.
- API payouts can only go to addresses in the project's address book. New API payouts wait in `pending-otp-verification` until the verify call. Payouts above the project limits also need approval in the dashboard with an emailed code. The default auto-approve amount is 0, so every payout needs an approval until you change it.
- There is no separate sandbox host. Use an install with `TURMITE_MODE=testnet`, or `turmite serve --demo` on your computer.

## Switch in steps

1. Point a staging copy of your shop at a testnet install.
2. Run your existing tests. Pay a few test orders with testnet coins.
3. Install the mainnet server and set its wallets and nodes.
4. Change the base URL, the API key and the IPN secret in production.
5. Pay one small real order on each network that you accept.

Payments that you created before the switch stay with the old gateway. Keep the old webhook handler and keys until those payments are final. To roll back, change the three settings back.
