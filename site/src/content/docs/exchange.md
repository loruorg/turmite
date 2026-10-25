---
title: Pay with any coin
description: Let customers pay with any coin that Trocador lists, while the payment arrives at its own deposit address in a coin that your project accepts.
sidebarTitle: Pay with any coin
---

The exchange option lets a customer pay with a coin that your project does not accept. Turmite uses [Trocador](https://trocador.app), an exchange aggregator, with your own Trocador API key. The exchange sends a coin that your project accepts to the payment's own deposit address. Turmite credits the payment when that coin arrives on chain.

A project with only one wallet can accept more than 200 coins this way. For example, a project with only a USDT wallet on Tron can accept BTC, XMR and every other coin that Trocador lists.

The exchange option is off by default. You turn it on per project, and it needs a Trocador API key.

## What the customer sees

1. On the invoice page, the customer clicks **Pay with another coin**. Turmite creates a payment in one of the project's coins. The payment page has the same link. It keeps the payment's coin.
2. The customer selects a coin from the list, or searches for it. Popular coins come first. The list shows 36 coins, and a search finds the others.
3. The customer can enter a **Refund address** for that coin. For a coin that uses a memo, the refund address is required, and the customer can also enter a **Refund memo**.
4. The customer clicks **Get the deposit address**.
5. The swap page shows the exact amount, the exchange's deposit address, its QR code and the memo if the coin needs one. It also shows the exchange, its KYC rating, the rate and the **Swap ID**.
6. The customer sends the coin to that address before the time on the page.
7. The status frame shows the swap in four steps. When the payment coin arrives, it shows the normal confirmation progress.

The swap page has the links **Choose another coin** and **Pay directly**. When a swap for the payment is open, the coin list shows a link to it.

The pages work with JavaScript off, as for a direct payment. See [Checkout](/docs/checkout/).

## The payment coin

On a payment page, the swap delivers the payment's own coin. On the invoice page, Turmite selects the first coin in this list that the project accepts and that Trocador lists:

`usdttrc20`, `usdtsol`, `usdcsol`, `usdcbase`, `usdtbsc`, `usdcbsc`, `usdtmatic`, `usdcmatic`, `usdc`, `usdterc20`, `btc`, `xmr`, `eth`, `ethbase`, `sol`, `trx`, `ton`, `bnbbsc`, `pol`

Turmite maps each ticker to Trocador's ticker and network. It uses the first pair that is in Trocador's coin list:

| Ticker | Trocador ticker and network |
|---|---|
| `btc` | `btc` Mainnet |
| `xmr` | `xmr` Mainnet |
| `eth` | `eth` ERC20, or `eth` Mainnet |
| `usdc` | `usdc` ERC20 |
| `usdterc20` | `usdt` ERC20 |
| `ethbase` | `eth` Base, or `ethbase` Mainnet |
| `usdcbase` | `usdc` Base |
| `pol` | `pol` Polygon, `pol` Mainnet, or `matic` Polygon |
| `usdcmatic` | `usdc` Polygon, or `usdc` MATIC |
| `usdtmatic` | `usdt` Polygon, or `usdt` MATIC |
| `bnbbsc` | `bnb` BEP20, or `bnb` BSC |
| `usdcbsc` | `usdc` BEP20, or `usdc` BSC |
| `usdtbsc` | `usdt` BEP20, or `usdt` BSC |
| `trx` | `trx` Mainnet, or `trx` TRC20 |
| `usdttrc20` | `usdt` TRC20 |
| `sol` | `sol` Mainnet, or `sol` Solana |
| `usdcsol` | `usdc` Solana, `usdc` SOL, or `usdc` SPL |
| `usdtsol` | `usdt` Solana, `usdt` SOL, or `usdt` SPL |
| `ton` | `ton` Mainnet, or `ton` TON |

The checkout does not offer the exchange for a coin that has no pair in the list.

## How it works

The `api` process sends the requests at checkout. All requests go to `https://api.trocador.app`, with your key in the `API-Key` header.

1. Turmite asks Trocador for a quote with `new_rate`. The request has the customer's coin as `ticker_from` and `network_from`, the payment coin as `ticker_to` and `network_to`, and `amount_to` set to the amount that is still due. It also has `payment=True`, `min_kycrating` and `markup`.
2. Turmite creates the swap with `new_trade`. The request has the quote ID, `fixed=True`, `payment=True`, `address` set to the payment's own deposit address, and the refund address if the customer gave one. It also has a webhook URL on your server: on the project's checkout domain when it has one, else on the site URL.
3. Trocador returns the exchange's deposit address (`address_provider`), its memo (`address_provider_memo`) and the amount to send (`amount_from`). Turmite refuses the swap when `amount_from` is less than the minimum or more than the maximum of the customer's coin in the coin list. The checkout shows the deposit address, the memo and the amount.
4. The exchange sends the payment coin to the deposit address.
5. The worker sees the transfer and counts the confirmations, as for a direct payment.
6. The payment becomes `finished`. Your shop receives the normal webhooks.

`payment=True` asks for a fixed amount out. The exchange must deliver `amount_to` to the deposit address. If the customer already paid part of the amount, `amount_to` is the rest.

The worker reads the status of each open swap with `trade`. It runs every 10 seconds and reads each swap at most every 20 seconds, for up to 3 days. A call from Trocador to the webhook URL makes Turmite read the status at once. Turmite does not trust the body of that call.

The worker also downloads Trocador's coin list once a day. After a failed download, it waits one hour.

The server must reach `https://api.trocador.app` on port 443. Allow this destination if your firewall limits outbound traffic.

## What Turmite trusts

Turmite credits only the coins that arrive at the deposit address on chain. Its own nodes must see the transfer with the network's required confirmations. Turmite never uses the Trocador status to credit a payment. The status is for the checkout display only.

- If less than the amount due arrives, the payment becomes `partially_paid`.
- If nothing arrives before the payment window closes, the payment becomes `expired`.
- If the swap fails, the payment stays unpaid. The customer gets a refund from the exchange.

## Turn it on

You need the `project_admin` role and a Trocador API key.

1. Get an API key from the Trocador partner program. See [trocador.app](https://trocador.app).
2. In the dashboard, open **Settings**, then the **Exchange** tab.
3. Select **Show "Pay with another coin" on the checkout**.
4. Enter the **Trocador API key**.
5. Select **Your markup** and **Lowest KYC rating**.
6. Optional: enter **Allowed coins**.
7. Enter a fresh code in **Two-factor code** and click **Save**.

On a mainnet install, Turmite checks a new key: it downloads the coin list with it. When Trocador refuses the key, Turmite does not save it.

Turmite seals the API key with `TURMITE_SECRET` before it stores it in the database. The dashboard never shows the key again. To change it, enter a new key. To remove it, use **Remove the API key** with a fresh two-factor code. This turns the exchange off.

### Settings

| Setting | Values | Effect |
|---|---|---|
| **Trocador API key** | Your key | Turmite sends it in the `API-Key` header of each Trocador request |
| **Your markup** | 0, 1, 1.65 or 3 %. Default 0 | Your commission on each swap |
| **Lowest KYC rating** | A, B, C or D. Default B | Trocador uses only exchanges with this rating or a better one |
| **Allowed coins** | Empty, or tickers separated by commas: `btc, xmr, ltc`. Add a network with a colon: `usdt:trc20` | Empty allows every coin that Trocador lists |

At 0 % markup, Trocador shares its fee with you. A higher markup gives customers worse rates. Trocador pays the commission under the terms of your Trocador account. Turmite does not record it.

### KYC rating

Trocador gives each exchange a KYC rating from A to D.

| Rating | Meaning |
|---|---|
| A | The exchange does not ask for identity checks |
| B, C | Ratings between A and D. Trocador publishes the criteria |
| D | The exchange can hold the customer's coins until the customer passes an identity check |

A strict rating protects your customers from held funds. It can also give fewer quotes and worse rates.

## Swap statuses

The status frame shows the Trocador status of the swap. The swap status does not change the payment status. Only the coins that arrive on chain change the payment status.

| Swap status | The frame shows |
|---|---|
| `new`, `waiting` | "Waiting for your {coin}", with the time left on the quote |
| `confirming` | "The exchange received your {coin}". The exchange waits for confirmations |
| `sending` | "The exchange is sending" |
| `finished` | "The exchange sent the coins". Turmite waits for them on the payment's network |
| `failed`, `halted` | "The swap failed", with the exchange name and the swap ID |
| `expired` | "The swap expired". No coins arrived in time |
| `refunded` | "The swap was refunded" |

After `failed`, `halted`, `expired` and `refunded`, the frame has a **Try again** button and a button for a direct payment, for example **Pay in USDT**. Trocador's "paid partially" shows as `confirming`. An unknown status shows as `waiting`.

## Refunds

The exchange makes refunds. Turmite does not. A refund goes to the refund address that the customer entered. Without a refund address, the customer must contact the exchange's support to get the coins back.

The customer must give the swap ID to the exchange's support. The swap page shows it.

To refund a payment that Turmite received, use a payout. See [Payouts](/docs/payouts/).

## Limits

- A customer can start a swap only while the payment is `waiting` or `partially_paid` and its window is open.
- Turmite does not hide coins whose minimum or maximum amount does not fit. When the exchange refuses the amount, the page shows an error. The customer can choose another coin or pay directly.
- Each quote has an expiry time. The customer must pay the exchange before that time.
- Some coins need a memo. The checkout shows it. The customer must send it with the transfer, or the exchange cannot match the deposit.
- A swap adds time. The payment window is 60 minutes by default. Set a window that leaves time for the swap and for the confirmations on both chains, in **Settings → General → Payment window (minutes)**. A deposit that arrives within one day after the window still counts. See [Payment statuses](/docs/statuses/).

## The payment object and webhooks

A payment through the exchange has the normal fields. `pay_currency`, `pay_amount` and `network` describe the coin that arrives at the deposit address. They do not describe the coin that the customer sent. The API, the webhooks and the dashboard do not show the swap. Webhooks are the same as for a direct payment. Ship on `finished`.

## Testnet installs

Trocador works with mainnet coins only. On a testnet install, the **Exchange** tab shows a notice, and customers do not see the exchange option. You can save the settings, but Turmite does not check the key.

To test the flow, use a mainnet install and a small real amount.

## Risks

- The exchange holds the customer's coins during the swap. A failed or halted swap costs the customer time or money until the exchange refunds. It costs you nothing, because Turmite credits only what arrives on chain.
- An exchange with rating C or D can hold the customer's coins for an identity check.
- The exchange rate and your markup change the amount that the customer pays. You receive the amount due.
- Trocador and the exchange see the customer's coin, the amount, the refund address and your deposit address.
- Your server sends the Trocador requests. The customer's browser does not contact Trocador.
