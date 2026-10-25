<?php
/**
 * Plugin Name:          Turmite for WooCommerce
 * Plugin URI:           https://turmite.com
 * Description:          Accept crypto payments in WooCommerce through your own Turmite server.
 * Version:              0.1.0
 * Requires at least:    6.5
 * Tested up to:         7.1
 * Requires PHP:         8.1
 * Requires Plugins:     woocommerce
 * Author:               Turmite
 * Author URI:           https://turmite.com
 * License:              MIT
 * License URI:          https://opensource.org/license/mit
 * Text Domain:          turmite-woocommerce
 * WC requires at least: 9.0
 * WC tested up to:      11.2
 *
 * @package Turmite_WooCommerce
 */

defined( 'ABSPATH' ) || exit;

define( 'TURMITE_WC_VERSION', '0.1.0' );
define( 'TURMITE_WC_FILE', __FILE__ );

add_action(
	'before_woocommerce_init',
	static function () {
		if ( class_exists( \Automattic\WooCommerce\Utilities\FeaturesUtil::class ) ) {
			\Automattic\WooCommerce\Utilities\FeaturesUtil::declare_compatibility( 'custom_order_tables', __FILE__, true );
			\Automattic\WooCommerce\Utilities\FeaturesUtil::declare_compatibility( 'cart_checkout_blocks', __FILE__, true );
		}
	}
);

add_action(
	'plugins_loaded',
	static function () {
		if ( ! class_exists( 'WC_Payment_Gateway' ) ) {
			return;
		}
		require_once __DIR__ . '/includes/class-turmite-signature.php';
		require_once __DIR__ . '/includes/class-turmite-api.php';
		require_once __DIR__ . '/includes/class-turmite-gateway.php';
		require_once __DIR__ . '/includes/class-turmite-ipn.php';

		add_filter(
			'woocommerce_payment_gateways',
			static function ( $gateways ) {
				$gateways[] = 'Turmite_Gateway';
				return $gateways;
			}
		);
		add_filter( 'woocommerce_payment_complete_order_status', array( 'Turmite_Gateway', 'filter_paid_status' ), 10, 3 );
		add_action( 'rest_api_init', array( 'Turmite_Ipn', 'register_routes' ) );
		add_filter(
			'plugin_action_links_' . plugin_basename( __FILE__ ),
			static function ( $links ) {
				$url = admin_url( 'admin.php?page=wc-settings&tab=checkout&section=turmite' );
				array_unshift( $links, '<a href="' . esc_url( $url ) . '">' . esc_html__( 'Settings', 'turmite-woocommerce' ) . '</a>' );
				return $links;
			}
		);
	}
);

add_action(
	'woocommerce_blocks_loaded',
	static function () {
		if ( ! class_exists( \Automattic\WooCommerce\Blocks\Payments\Integrations\AbstractPaymentMethodType::class ) ) {
			return;
		}
		require_once __DIR__ . '/includes/class-turmite-blocks.php';
		add_action(
			'woocommerce_blocks_payment_method_type_registration',
			static function ( $registry ) {
				$registry->register( new Turmite_Blocks() );
			}
		);
	}
);
