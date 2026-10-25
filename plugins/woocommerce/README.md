# Turmite for WooCommerce

WooCommerce payment gateway for a self-hosted Turmite server. PHP 8.1+, WordPress 6.5+, WooCommerce 9.0+. Supports the Cart and Checkout blocks and HPOS.

## Install

Run `./build.sh`, then upload `dist/turmite-woocommerce.zip` in Plugins > Add New > Upload Plugin.

## Settings

WooCommerce > Settings > Payments > Turmite:

- Turmite URL: the site URL of your Turmite server, for example `https://pay.example.com`.
- API key and IPN secret: from your project in the Turmite dashboard.
- Signature header: the same name as in the project settings. Default `x-turmite-sig`.
- Order status on payment: the status of the order when the payment is `finished`.

## IPN URL

`https://your-store.example/wp-json/turmite/v1/ipn`. The plugin sends it with each invoice. Turmite must be able to reach it.

## Tests

```sh
docker run --rm -v "$PWD":/p -w /p php:8.5-cli php tests/signature-test.php
```
