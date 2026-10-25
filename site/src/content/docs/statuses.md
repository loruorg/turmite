---
title: Payment statuses
description: Every payment and payout status, what it means and what your shop does.
sidebarTitle: Statuses
---

A payment has one status in `payment_status`. Turmite sends a webhook at each change. This page lists each status and what your shop does. Payout statuses are at the end.

## Payment statuses

| Status | Meaning | Final | What your shop does |
|---|---|---|---|
| `waiting` | The payment exists. No deposit is seen yet | No | Show the payment page |
| `confirming` | A deposit is seen on the chain. It does not have the required confirmations yet | No | Show "processing". Do not ship |
| `confirmed` | The confirmed deposits cover the amount | No | Wait for `finished`. It usually follows in the same worker pass |
| `partially_paid` | All deposits are confirmed, but they are less than the amount | No | Wait. The customer can send the rest |
| `finished` | The payment is complete | Yes | Ship the order |
| `refunded` | A project admin marked the payment as refunded | Yes | Cancel the order |
| `expired` | The payment window closed without enough funds | Yes | Cancel the order or create a new payment |
| `failed` | Turmite does not set this status. It is in the API for compatibility | Yes | Do not ship |
| `sending` | Turmite does not set this status. It is in the API for compatibility | No | Wait for `finished` |

Ship only on `finished`. The `isPaid()` function of the [TypeScript SDK](/docs/sdk/) returns true only for `finished`.

A payment counts as paid when the confirmed amount is at least 99 % of `pay_amount`. This tolerance absorbs wallet rounding. Compare `actually_paid` with `pay_amount` if you need the exact amount.

### Partial payments

A payment is `partially_paid` when its deposits are less than 99 % of `pay_amount` and all of them have the required confirmations. The checkout page shows "Partly paid" and the amount still due. The customer sends the rest to the same address. When the total is enough, the payment continues to `confirmed` and `finished`.

Turmite watches the address for one day after the payment window closes. A payment that is still short at that time becomes `expired`. The funds that arrived go to your wallet. Decide by hand: refund with a payout, or accept the order with **Mark as paid**.

### Payments per chain

The payment becomes `finished` as soon as its confirmed deposits cover the amount. On the swept chains, Turmite then moves the funds from the deposit address to your wallet. The sweep does not change the payment status. `payout_hash` shows the sweep transaction.

| Network | After `finished` |
|---|---|
| Bitcoin, Monero | Nothing. The funds are already in your wallet |
| Ethereum, Base, Polygon, BNB Smart Chain | The deposit contract flushes to your cold wallet |
| Tron, Solana, TON | The worker sweeps the deposit key to your cold wallet |

### State diagram

```text
waiting -> confirming -> confirmed -> finished
waiting        -> confirmed        the first deposit already has its confirmations
waiting        -> expired          no deposit before the payment window closes
confirming     -> partially_paid   less than the amount, all of it confirmed
partially_paid -> confirming       the customer sends the rest
partially_paid -> expired          still short one day after the window closes
expired        -> confirming       a deposit arrives within one day after the window
any status except finished and refunded -> finished   Mark as paid
finished, partially_paid, failed, expired with a deposit -> refunded   Mark as refunded
```

After `finished`, a further deposit within one day after the window sends the `finished` webhook again with the new `actually_paid`. When the deposit is confirmed, the webhook comes again with the new `outcome_amount`.

### Expiry

`expiration_estimate_date` is the end of the payment window. The project setting **Payment window (minutes)** sets it. The default is 60 minutes. The allowed range is 5 to 10080 minutes (7 days). The countdown on the payment page uses the same time.

A deposit that arrives within one day after the window still counts. The payment continues to `finished`, and the dashboard shows "Paid late" in the **Settlement** field of the payment. Check late payments before you ship, because the price can have changed.

### Manual actions

On the payment page in the dashboard, a project admin can change a payment by hand under **Manual actions**. Each action needs a fresh two-factor code, writes an audit log entry and sends a webhook.

| Action | Shown for | New status | Use it when |
|---|---|---|---|
| **Mark as paid** | Every status except `finished` and `refunded` | `finished` | The customer paid in another way, or you checked the funds yourself |
| **Mark as refunded** | `finished`, `partially_paid`, `failed`, and `expired` when a deposit arrived | `refunded` | You sent the funds back with a payout or from your wallet |

Turmite does not send funds back by itself.

### Webhook retries per status

Turmite retries a webhook that does not get a `2xx` response:

| Kind | Statuses | Attempts |
|---|---|---|
| Progress | `waiting`, `confirming`, `confirmed`, `partially_paid` | 3: now, 10 s, 60 s |
| Final | `finished`, `refunded`, `expired` | 11: now, 10 s, 1 m, 5 m, 30 m, 1 h, 2 h, 4 h, 8 h, 24 h, 48 h |

See [Webhooks](/docs/webhooks/#retries).

## Payout statuses

A payout has one of these statuses. Each status change after creation sends a `payout.<status>` webhook.

| Status | Meaning | Final |
|---|---|---|
| `pending-otp-verification` | An API batch waits for `POST /v1/payout/{batch_id}/verify`, or an admin approval waits for the emailed code | No |
| `pending-approval` | A project limit holds the payout until a project admin approves it | No |
| `pending` | Cleared to send. It waits for the worker | No |
| `signing` | The worker signs the transaction and stores it before broadcast | No |
| `initiated` | A node accepted the transaction. It waits for confirmations | No |
| `sent` | The transaction has the required confirmations | Yes |
| `failed` | The transaction did not happen | Yes |
| `rejected` | A project admin rejected the payout | Yes |
| `cancelled` | A project admin cancelled the payout before signing | Yes |
| `needs-review` | Turmite cannot tell whether the transaction landed. A project admin must check and resolve it | No |

Do not create the same payout again while it is `needs-review`. A second payout can pay twice. See [Payouts](/docs/payouts/#needs-review).
