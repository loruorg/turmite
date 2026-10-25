<?php
/**
 * Cart and Checkout blocks support.
 *
 * @package Turmite_WooCommerce
 */

defined( 'ABSPATH' ) || exit;

use Automattic\WooCommerce\Blocks\Payments\Integrations\AbstractPaymentMethodType;

final class Turmite_Blocks extends AbstractPaymentMethodType {

	protected $name = 'turmite';

	public function initialize() {
		$settings       = get_option( 'woocommerce_turmite_settings', array() );
		$this->settings = is_array( $settings ) ? $settings : array();
	}

	public function is_active() {
		$gateways = WC()->payment_gateways()->payment_gateways();
		return isset( $gateways['turmite'] ) && $gateways['turmite']->is_available();
	}

	public function get_payment_method_script_handles() {
		wp_register_script(
			'turmite-blocks',
			plugins_url( 'assets/js/blocks.js', TURMITE_WC_FILE ),
			array( 'wc-blocks-registry', 'wc-settings', 'wp-element', 'wp-html-entities', 'wp-i18n' ),
			TURMITE_WC_VERSION,
			true
		);
		wp_set_script_translations( 'turmite-blocks', 'turmite-woocommerce' );
		return array( 'turmite-blocks' );
	}

	public function get_payment_method_data() {
		return array(
			'title'       => $this->get_setting( 'title' ),
			'description' => $this->get_setting( 'description' ),
			'supports'    => array( 'products' ),
		);
	}
}
