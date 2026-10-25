---
title: WooCommerce plugin
description: Install the Turmite payment gateway plugin for WooCommerce and connect it to your Turmite server.
sidebarTitle: WooCommerce
---

The WooCommerce plugin adds Turmite as a payment method in your store. The customer pays on the Turmite checkout. A signed IPN updates the order. The source is in `plugins/woocommerce` in the repository.

## Requirements

- WordPress 6.5 or later.
- WooCommerce 9.0 or later.
- PHP 8.1 or later.
- A Turmite server that your store can reach over `https`.
- An API key and the IPN secret of a Turmite project.

Plugin version 0.1.0 is tested with WordPress 7.1 and WooCommerce 11.2. The plugin works with the classic checkout, the Cart and Checkout blocks, and High-Performance Order Storage (HPOS).

## Install

1. Download `turmite-woocommerce.zip` from the latest release on [GitHub](https://github.com/loruorg/turmite/releases).
2. In WordPress, open **Plugins** → **Add New Plugin** → **Upload Plugin**.
3. Select the zip file and click **Install Now**.
4. Click **Activate**.

To build the zip from source, run `./build.sh` in `plugins/woocommerce`. The script writes `dist/turmite-woocommerce.zip`.

## Connect the plugin

1. In Turmite, create an API key for the project. See [Merchant API](/docs/merchant-api/#authentication).
2. Copy the project IPN secret. See [Webhooks](/docs/webhooks/#the-secrets).
3. In WordPress, open **WooCommerce** → **Settings** → **Payments** → **Turmite**.
4. Fill in the settings and save.

Customers see the payment method only when **Turmite URL**, **API key** and **IPN secret** are set.

| Setting | Value |
|---|---|
| Enable | On ("Accept crypto payments through Turmite") |
| Title | Name of the payment method at checkout. The default is "Crypto" |
| Description | Text under the payment method at checkout |
| Turmite URL | Your Turmite site URL, for example `https://pay.example.com`. It can have a path. It must start with `https://` or `http://` |
| API key | The project API key (`tm_…`) |
| IPN secret | The project IPN secret |
| Signature header | `x-turmite-sig` by default. Change it only if you changed **IPN signature header** in Turmite **Settings** |
| Order status on payment | The order status when the payment is `finished`: WooCommerce default, Processing, Completed or On hold |
| Debug log | Writes a debug log in **WooCommerce** → **Status** → **Logs**, source `turmite`. The plugin always logs errors. The log contains no secrets |

## How an order is paid

1. The customer selects Turmite at checkout and places the order.
2. The plugin creates an invoice with `POST /v1/invoice`. It sends the WooCommerce order ID as `order_id`, `Order #<order number>` as `order_description`, the order total and the store currency as the price, the IPN URL, `success_url` (the order received page) and `cancel_url` (the checkout page).
3. The plugin sends the customer to `invoice_url`.
4. The customer pays on the Turmite checkout.
5. Turmite sends a signed IPN to the store at each status change.
6. The plugin checks the signature in the signature header. When `x-turmite-timestamp` and `x-turmite-signature` are present, it also checks them and refuses a timestamp more than 300 seconds from its clock. Then it updates the order.
7. The "Return to {store}" button opens the order received page.

The IPN URL is `https://shop.example.com/wp-json/turmite/v1/ipn`. The plugin settings page shows it.

- The plugin ignores a repeated IPN for the same payment and status. A `finished` IPN with a new `actually_paid` only adds a note.
- When the price, the currency or the invoice of an IPN does not agree with the order, the plugin puts an unpaid order on hold and adds a note.
- An IPN for an order that does not exist gets `404`, so Turmite retries it.
- An IPN for an older payment of the same order changes the order only when it is `finished`.

## Order status mapping

| Turmite status | WooCommerce order |
|---|---|
| `waiting`, `confirming`, `confirmed` | No change. The plugin adds an order note |
| `partially_paid` | On hold, with a note of the amount received. A paid order only gets a note |
| `finished` | Paid. The status is the one in **Order status on payment**. An order that is already paid only gets a note. The note warns of a double payment when another payment of the order finished |
| `expired` | Cancelled, if the order is still pending |
| `refunded` | Refunded, if the order is paid or on hold |
| `failed` | Failed, if the order is still pending. Turmite does not set this status now |

With **WooCommerce default**, a paid order becomes Processing, or Completed when all items are virtual and downloadable.

## Test the plugin

1. Connect the store to a Turmite install with `TURMITE_MODE=testnet`.
2. Place an order and pay with testnet coins.
3. Check that the order gets the status of **Order status on payment**, by default Processing.
4. In Turmite, open **Webhooks** and check the response code under **Deliveries**.

On mainnet, the IPN URL must be a public `https` URL. Turmite does not follow redirects, so the URL must answer directly.

## Troubleshooting

| Problem | Check |
|---|---|
| The order stays Pending payment | Open **Webhooks** in Turmite. Look at the response code of the IPN under **Deliveries** |
| The IPN gets `401` | The IPN secret or the signature header in the plugin does not match Turmite, or the clock of the store is wrong |
| The IPN gets `404` | The order does not exist in this store |
| The IPN gets `301` or `302` | The store redirects the IPN URL. Use the final URL, for example with `https://` and the right `www` prefix |
| The checkout shows an error | The base URL or the API key is wrong, or the ticker is not enabled for the project |
