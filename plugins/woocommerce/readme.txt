=== Turmite for WooCommerce ===
Contributors: turmite
Tags: crypto, bitcoin, payments, payment gateway, woocommerce
Requires at least: 6.5
Tested up to: 7.1
Requires PHP: 8.1
Stable tag: 0.1.0
License: MIT
License URI: https://opensource.org/license/mit

Accept crypto payments in WooCommerce through your own Turmite server.

== Description ==

Turmite is a free, self-hosted crypto payment gateway. This plugin connects a WooCommerce store to your Turmite server.

At checkout, the plugin creates a Turmite invoice for the order and sends the customer to the invoice page. The customer selects a coin and pays. Turmite sends signed payment updates (IPN) to the store. The plugin verifies each signature and updates the order.

The coins are the ones that are enabled in your Turmite project.

The plugin supports the classic checkout, the Cart and Checkout blocks, and High-Performance Order Storage.

Order status for each payment status:

* finished: the payment is complete. The order gets the status that you set in "Order status on payment".
* partially_paid: on hold.
* expired: cancelled, if the order is pending.
* failed: failed, if the order is pending.
* refunded: refunded.
* waiting, confirming, confirmed, sending: an order note only.

When the amount, the currency or the invoice of an update does not agree with the order, the order goes on hold.

The plugin sends data only to the Turmite server that you set. It contains no tracking.

== Installation ==

1. Install Turmite on your server. See https://turmite.com.
2. In the Turmite dashboard, open your project. Make an API key and copy the IPN secret.
3. In WordPress, go to Plugins > Add New > Upload Plugin. Upload the zip file and activate the plugin.
4. Go to WooCommerce > Settings > Payments > Turmite. Set the Turmite URL, the API key and the IPN secret. Enable the payment method.

== Frequently Asked Questions ==

= Which IPN URL does Turmite use? =

The plugin sends the URL with each invoice. It is https://your-store.example/wp-json/turmite/v1/ipn. The settings page shows it. On mainnet, Turmite accepts only public HTTPS URLs.

= When is an order paid? =

When the Turmite payment status is "finished".

= Can I use a different signature header name? =

Yes. Set the same name in the Turmite project settings and in the plugin settings.

= Does the store hold keys or funds? =

No. The plugin keeps only the API key and the IPN secret.

== Changelog ==

= 0.1.0 =
* First release.
