---
title: Payouts
description: Send funds from a hot wallet for refunds and other payments, with an address book, limits and two-factor approval.
sidebarTitle: Payouts
---

A payout sends funds from a hot wallet to an address. Use payouts for refunds, supplier payments and withdrawals. You create payouts in the dashboard or with the merchant API.

## Networks

| Network | Payouts |
|---|---|
| Ethereum, Base, Polygon, BNB Smart Chain | Yes: the native coin and ERC-20 tokens |
| Solana | Yes: SOL and SPL tokens |
| Tron | Yes: TRX and TRC-20 tokens |
| TON | Native TON only. Turmite refuses jetton payouts |
| Bitcoin | No. Turmite has no Bitcoin key |
| Monero | No. Turmite has a view key only |

A payout on Bitcoin or Monero gets `400`.

## How hot wallets work

A hot wallet is a key that Turmite holds. It pays payouts and the network fees of sweeps.

- The `api` process encrypts each generated or imported key to `TURMITE_HOT_RECIPIENT`. It cannot decrypt keys.
- Only the `worker` process can decrypt hot keys. It needs `TURMITE_HOT_IDENTITY_FILE` and the passphrase.
- The worker opens no port.
- Payouts use only the nodes that connected in the worker. A node on another network (for example, a testnet node on a mainnet install) does not connect.

A full server compromise can spend the balance of the hot wallets. Keep that balance small. See [Security model](/docs/security-model/).

## Set up payouts

1. A global admin opens **Hot wallets** in the **Server** section of the dashboard menu.
2. Under **Make a hot wallet** or **Import a key**, select the family (EVM, Tron, Solana or TON) and enter a label and a fresh two-factor code. One EVM key works on every EVM network.
3. Under **Assign to a project**, select the project, the family and the hot wallet. Enter a two-factor code and click **Save**.
4. Send the native coin for network fees to the hot wallet address. Send the tokens that you plan to pay out.
5. In the project, open **Payouts** → **Limits** and set the limits.
6. Open **Payouts** → **Address book** and add the destinations for API payouts.

A project without a hot wallet for a network cannot send payouts on it. The request gets `400`.

## Limits

All limits are in USD, at the rate when the payout is created. A project admin sets them under **Payouts** → **Limits** with a fresh two-factor code. Each limit is 0 by default.

| Limit | Effect | 0 means |
|---|---|---|
| **Minimum payout** | Turmite refuses a smaller payout with `400` | No minimum |
| **Approve automatically up to** | A larger payout waits for an admin approval | Every payout waits for an approval |
| **Hourly limit** | A payout that takes the total of the last hour above this amount waits for an approval | No limit |
| **Daily limit** | A payout that takes the total of the last 24 hours above this amount waits for an approval | No limit |

The totals count every payout of the project that was cleared to send in the window: `pending`, `signing`, `initiated`, `sent` and `needs-review`. They include all users and API keys. The **Limits** page shows the totals of the last hour and the last 24 hours.

When no USD rate is available, the payout waits for an approval. A project with a minimum refuses the payout with `503`, because it cannot check the minimum.

## Address book

The address book holds the approved destinations of the project. Only a project admin can change it, in the dashboard, with a fresh two-factor code. Each entry has a label (1 to 80 characters), a network, an address and an optional memo (up to 120 characters). An EVM address in mixed case must have a valid checksum.

API payouts can only go to addresses in the address book:

- The address must be an entry on the network of the ticker.
- With `extra_id`, an entry with the same memo must exist.
- Without `extra_id`, Turmite uses the memo of the only entry for that address. If the address has several entries and all of them have a memo, the request gets `400`. Send `extra_id`.

Payouts made in the dashboard can go to any valid address. When you remove an entry, API payouts to that address stop.

## Create a payout in the dashboard

A project admin can create a payout.

1. Open **Payouts** → **New payout**.
2. Select the **Currency**. Enter the **Address** or select an entry under **From the address book**.
3. Enter the **Amount**, and a **Memo** and a **Description** if necessary.
4. Enter a fresh two-factor code and click **Create payout**.

Turmite checks the address for the network. It refuses an EVM address with a wrong checksum. The limits apply at once: the payout is `pending`, or `pending-approval` when a limit holds it.

## Create a payout with the API

The API needs the project API key and a token for a dashboard user. The user must be a project admin of the key's project, or a global admin. On a mainnet install, the user must have two-factor authentication on.

1. Get a token. `POST /v1/auth` takes the user's email address and password. The token is valid for 5 minutes.

   ```bash
   TOKEN=$(curl -s -X POST https://pay.example.com/v1/auth \
     -H "Content-Type: application/json" \
     -d '{"email": "ops@example.com", "password": "..."}' | jq -r .token)
   ```

2. Create a batch. A batch holds 1 to 100 withdrawals.

   ```bash
   curl -X POST https://pay.example.com/v1/payout \
     -H "x-api-key: $TM_API_KEY" \
     -H "Authorization: Bearer $TOKEN" \
     -H "Content-Type: application/json" \
     -d '{
       "withdrawals": [
         {
           "address": "TQrZ9wBzhXnHdAdKwRBfdMH2RiTjH6yGtW",
           "currency": "usdttrc20",
           "amount": "250.00",
           "ipn_callback_url": "https://shop.example.com/turmite/payout-ipn",
           "unique_external_id": "refund-1042"
         }
       ]
     }'
   ```

   ```json
   {
     "id": "5000000713",
     "withdrawals": [
       {
         "id": "5000248325",
         "batch_withdrawal_id": "5000000713",
         "address": "TQrZ9wBzhXnHdAdKwRBfdMH2RiTjH6yGtW",
         "extra_id": null,
         "currency": "usdttrc20",
         "network": "tron",
         "amount": "250",
         "fee": null,
         "fee_currency": "trx",
         "status": "pending-otp-verification",
         "hash": null,
         "error": null,
         "ipn_callback_url": "https://shop.example.com/turmite/payout-ipn",
         "payout_description": null,
         "unique_external_id": "refund-1042",
         "created_at": "2026-10-23T13:00:00.000Z",
         "requested_at": null,
         "updated_at": "2026-10-23T13:00:00.000Z"
       }
     ]
   }
   ```

   The answer is `201`. New payouts have the status `pending-otp-verification` until you verify the batch.

3. Verify the batch with the current two-factor code of the token's user.

   ```bash
   curl -X POST https://pay.example.com/v1/payout/5000000713/verify \
     -H "x-api-key: $TM_API_KEY" \
     -H "Authorization: Bearer $TOKEN" \
     -H "Content-Type: application/json" \
     -d '{"verification_code": "123456"}'
   ```

After the verification, the limits decide. A payout within the limits goes to `pending`. A payout that a limit holds goes to `pending-approval` until an admin approves it in the dashboard. The response is the batch with the new statuses.

A two-factor code works one time. After 5 wrong codes, the user gets `429` for 15 minutes. On a testnet install, a user without two-factor authentication can verify without a code.

### Fields of a withdrawal

| Field | Required | Description |
|---|---|---|
| `address` | yes | Destination. It must be in the address book |
| `currency` | yes | Ticker, for example `usdttrc20` |
| `amount` | yes | Amount in `currency`. It cannot have more decimals than the ticker |
| `extra_id` | no | Memo, up to 120 characters. Turmite sends it as a comment on TON and as a memo on Tron |
| `ipn_callback_url` | no | URL that receives the webhooks of this payout |
| `payout_description` | no | Text for your records, up to 500 characters |
| `unique_external_id` | no | Your idempotency ID, up to 255 characters |

An `ipn_callback_url` at the top level of the request applies to each withdrawal that has none.

`unique_external_id` makes a withdrawal idempotent. Send the same request again when you retry after a timeout. A second request with the same `unique_external_id` and the same ticker, address, memo and amount returns the existing payout. The same `unique_external_id` with other fields returns `409` with the code `CONFLICT`. The same `unique_external_id` twice in one request returns `400`.

### Read payouts

`GET /v1/payout/{batch_id}` returns one batch. It needs the API key only.

`GET /v1/payout` returns a page of payouts in `payouts`, with `limit`, `page`, `pagesCount` and `total`. It needs the API key and a token of a project member.

| Parameter | Description |
|---|---|
| `batch_id` | Only payouts of this batch |
| `status` | Only payouts with this status |
| `date_from`, `date_to` | Range of `created_at`. A date or a date-time |
| `order` | `asc` or `desc`. The default is `desc` |
| `limit` | Payouts per page, from 1 to 500. The default is 10 |
| `page` | Page number. The first page is 0 |

## The payout object

| Field | Description |
|---|---|
| `id` | Payout ID, numeric text |
| `batch_withdrawal_id` | ID of the batch, numeric text |
| `address`, `extra_id` | Destination and its memo |
| `currency` | Ticker |
| `network` | Network code in lowercase, for example `tron` |
| `amount` | Amount, a decimal string |
| `fee` | Network fee in `fee_currency` as a decimal string, or `null` before Turmite knows it. The hot wallet pays it |
| `fee_currency` | Native coin of the network, for example `trx` |
| `status` | See [Statuses](#statuses) |
| `hash` | Transaction hash, or `null` |
| `error` | Error text, or `null` |
| `ipn_callback_url` | URL that receives the payout webhooks |
| `payout_description` | Your text |
| `unique_external_id` | Your idempotency ID |
| `created_at` | Time of creation |
| `requested_at` | Time when a node first accepted the transaction, or `null` |
| `updated_at` | Time of the last change |

## Approve a payout

A payout that waits for approval has the status `pending-approval`. A project admin approves it in two steps.

1. Open **Payouts** and select the payout.
2. Under **Approve**, enter a fresh two-factor code and click **Email me a code**. Turmite emails a 6-digit code to that admin. The payout goes to `pending-otp-verification`.
3. Enter the code under **Code from the email** and click **Approve payout**. The payout goes to `pending`.

The emailed code works for 10 minutes and for 5 attempts. Only the admin who received it can enter it. When the code expires or the last attempt fails, the payout goes back to `pending-approval`. To get a new code, click **Send a new code**. Another admin can start a new approval. The new code replaces the old one.

Under **Stop this payout**, click **Reject** to end a payout that waits for approval or verification. Click **Cancel payout** to stop a `pending` payout before Turmite signs it. Both need a two-factor code and are final.

The emailed code needs SMTP. Set `TURMITE_SMTP_URL` and `TURMITE_SMTP_FROM`. See [Configuration](/docs/configuration/). On testnet without SMTP, Turmite writes the email to the log. On mainnet without SMTP, the email cannot be sent, so held payouts cannot be approved.

## Statuses

```text
API batch:  pending-otp-verification --verify--> pending or pending-approval
Dashboard:  pending or pending-approval
Approval:   pending-approval -> pending-otp-verification -> pending
Worker:     pending -> signing -> initiated -> sent
Errors:     pending, signing or initiated -> failed
            initiated -> needs-review -> sent or failed
Stop:       pending-approval or pending-otp-verification -> rejected
            pending -> cancelled
```

[Payment statuses](/docs/statuses/#payout-statuses) describes each status.

Each status change after creation sends a `payout.<status>` webhook to the project endpoints and to the withdrawal's `ipn_callback_url`. The `ipn_callback_url` gets the flat payout object, signed with the project IPN secret. See [Webhooks](/docs/webhooks/).

## Needs review

`needs-review` means that Turmite cannot tell whether the transaction happened. For example, the nonce of the hot wallet was used by a transaction that Turmite does not know. Turmite does not guess. A wrong `failed` status can cause a second payment.

1. Open the hot wallet address in a block explorer.
2. Look for a transfer of that amount to that address.
3. Open the payout in **Payouts**. Under **Resolve**, select **It was sent** or **It was not sent**.
4. Write a note of what you checked, for example the transaction hash. Enter a two-factor code and click **Save decision**.

The payout becomes `sent` or `failed`. The decision is final and sends the final webhook. Do not create the payout again until you resolve it.

## Check before you send

`POST /v1/payout/validate-address` checks an address for a ticker with the checks of the wallet forms: EIP-55 for mixed-case EVM addresses, the checksum and the network of Bitcoin addresses, the checksum of Tron addresses, the length of Solana keys and the network prefix of Monero addresses. Then it asks the node, when one is connected. A valid address returns `200` with the body `"OK"`. An address that is not valid returns `400` with the code `INVALID_REQUEST_PARAMS`.

```bash
curl -X POST https://pay.example.com/v1/payout/validate-address \
  -H "x-api-key: $TM_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"address": "TQrZ9wBzhXnHdAdKwRBfdMH2RiTjH6yGtW", "currency": "usdttrc20"}'
```

`GET /v1/payout/fee` estimates the network fee. `amount` is optional. Turmite copies it to the response.

```bash
curl -H "x-api-key: $TM_API_KEY" \
  "https://pay.example.com/v1/payout/fee?currency=usdttrc20&amount=250"
```

```json
{"currency": "usdttrc20", "amount": "250", "fee": 1.1, "network_fee": 4.2, "fee_currency": "trx"}
```

`network_fee` is the fee in the native coin, `fee_currency`. `fee` is the same fee converted to `currency` at the USD rates. The hot wallet pays the fee in the native coin. Turmite records the real fee in the payout when it sends it.

## Good practice

- Keep about one week of fees and payouts in each hot wallet. Refill it from your cold wallet.
- Set a low auto-approve amount and low hourly and daily limits.
- Give `project_admin` only to people who can approve payouts.
- Do not use a hot wallet key in other software. Turmite must be the only sender from that address.
- Keep the worker passphrase out of backups that other people can read.
