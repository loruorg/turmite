<?php
/**
 * The Turmite payment gateway.
 *
 * @package Turmite_WooCommerce
 */

defined( 'ABSPATH' ) || exit;

class Turmite_Gateway extends WC_Payment_Gateway {

	public const PAID_STATUSES = array( 'processing', 'completed', 'on-hold' );

	public function __construct() {
		$this->id                 = 'turmite';
		$this->method_title       = __( 'Turmite', 'turmite-woocommerce' );
		$this->method_description = __( 'Customers pay with crypto on the invoice page of your Turmite server.', 'turmite-woocommerce' );
		$this->has_fields         = false;
		$this->supports           = array( 'products' );

		$this->init_form_fields();
		$this->init_settings();

		$this->title       = $this->get_option( 'title' );
		$this->description = $this->get_option( 'description' );

		add_action( 'woocommerce_update_options_payment_gateways_' . $this->id, array( $this, 'process_admin_options' ) );
		add_action( 'woocommerce_thankyou_' . $this->id, array( $this, 'thankyou_page' ) );
	}

	public static function ipn_url(): string {
		return rest_url( 'turmite/v1/ipn' );
	}

	public function get_method_description() {
		$text = $this->method_description . ' ' . sprintf(
			/* translators: %s: the IPN URL of this store */
			__( 'Turmite sends payment updates to %s. Your Turmite server must be able to reach this URL.', 'turmite-woocommerce' ),
			'<code>' . esc_html( self::ipn_url() ) . '</code>'
		);
		return apply_filters( 'woocommerce_gateway_method_description', $text, $this );
	}

	public function init_form_fields() {
		$this->form_fields = array(
			'enabled'     => array(
				'title'   => __( 'Enable', 'turmite-woocommerce' ),
				'type'    => 'checkbox',
				'label'   => __( 'Accept crypto payments through Turmite', 'turmite-woocommerce' ),
				'default' => 'no',
			),
			'title'       => array(
				'title'       => __( 'Title', 'turmite-woocommerce' ),
				'type'        => 'text',
				'description' => __( 'The name of the payment method at checkout.', 'turmite-woocommerce' ),
				'default'     => __( 'Crypto', 'turmite-woocommerce' ),
				'desc_tip'    => true,
			),
			'description' => array(
				'title'       => __( 'Description', 'turmite-woocommerce' ),
				'type'        => 'textarea',
				'description' => __( 'The text below the payment method at checkout.', 'turmite-woocommerce' ),
				'default'     => __( 'Pay with cryptocurrency. The next page shows the amount and the address.', 'turmite-woocommerce' ),
				'desc_tip'    => true,
			),
			'api_url'     => array(
				'title'       => __( 'Turmite URL', 'turmite-woocommerce' ),
				'type'        => 'text',
				'description' => __( 'The site URL of your Turmite server. It can have a path.', 'turmite-woocommerce' ),
				'placeholder' => 'https://pay.example.com',
			),
			'api_key'     => array(
				'title'       => __( 'API key', 'turmite-woocommerce' ),
				'type'        => 'password',
				'description' => __( 'A project API key from the Turmite dashboard. It starts with tm_.', 'turmite-woocommerce' ),
			),
			'ipn_secret'  => array(
				'title'       => __( 'IPN secret', 'turmite-woocommerce' ),
				'type'        => 'password',
				'description' => __( 'The IPN secret of the same project. The plugin uses it to verify each payment update.', 'turmite-woocommerce' ),
			),
			'sig_header'  => array(
				'title'       => __( 'Signature header', 'turmite-woocommerce' ),
				'type'        => 'text',
				'description' => __( 'The signature header name in the project settings.', 'turmite-woocommerce' ),
				'default'     => 'x-turmite-sig',
			),
			'paid_status' => array(
				'title'       => __( 'Order status on payment', 'turmite-woocommerce' ),
				'type'        => 'select',
				'default'     => '',
				'options'     => array(
					''           => __( 'WooCommerce default', 'turmite-woocommerce' ),
					'processing' => __( 'Processing', 'turmite-woocommerce' ),
					'completed'  => __( 'Completed', 'turmite-woocommerce' ),
					'on-hold'    => __( 'On hold', 'turmite-woocommerce' ),
				),
				'description' => __( 'The order status when the payment is finished. The WooCommerce default is Processing, or Completed when all items are virtual and downloadable.', 'turmite-woocommerce' ),
			),
			'debug'       => array(
				'title'       => __( 'Debug log', 'turmite-woocommerce' ),
				'type'        => 'checkbox',
				'label'       => __( 'Write a debug log', 'turmite-woocommerce' ),
				'default'     => 'no',
				'description' => __( 'The log is in WooCommerce > Status > Logs, source "turmite". Errors are always logged. The log contains no secrets.', 'turmite-woocommerce' ),
			),
		);
	}

	public function validate_api_url_field( $key, $value ) {
		$value = untrailingslashit( trim( (string) $value ) );
		if ( '' === $value ) {
			return '';
		}
		if ( ! in_array( wp_parse_url( $value, PHP_URL_SCHEME ), array( 'http', 'https' ), true ) || ! wp_parse_url( $value, PHP_URL_HOST ) ) {
			throw new Exception( __( 'The Turmite URL must start with https:// or http://.', 'turmite-woocommerce' ) );
		}
		return esc_url_raw( $value );
	}

	public function validate_sig_header_field( $key, $value ) {
		$value = strtolower( trim( (string) $value ) );
		if ( '' === $value ) {
			return 'x-turmite-sig';
		}
		if ( 1 !== preg_match( '/^[a-z0-9-]+$/', $value ) ) {
			throw new Exception( __( 'The signature header name can contain only letters, digits and hyphens.', 'turmite-woocommerce' ) );
		}
		return $value;
	}

	public function process_admin_options() {
		$saved = parent::process_admin_options();
		foreach ( $this->get_errors() as $error ) {
			WC_Admin_Settings::add_error( $error );
		}
		return $saved;
	}

	public function is_available() {
		foreach ( array( 'api_url', 'api_key', 'ipn_secret' ) as $key ) {
			if ( '' === (string) $this->get_option( $key ) ) {
				return false;
			}
		}
		return parent::is_available();
	}

	public function process_payment( $order_id ) {
		$order = wc_get_order( $order_id );
		if ( ! $order instanceof WC_Order ) {
			wc_add_notice( __( 'The order was not found.', 'turmite-woocommerce' ), 'error' );
			return array( 'result' => 'failure' );
		}

		$request = array(
			'price_amount'      => wc_format_decimal( $order->get_total(), wc_get_price_decimals() ),
			'price_currency'    => strtolower( $order->get_currency() ),
			'order_id'          => (string) $order->get_id(),
			/* translators: %s: order number */
			'order_description' => sprintf( __( 'Order #%s', 'turmite-woocommerce' ), $order->get_order_number() ),
			'ipn_callback_url'  => self::ipn_url(),
			'success_url'       => $this->get_return_url( $order ),
			'cancel_url'        => wc_get_checkout_url(),
		);

		try {
			$api     = new Turmite_Api( (string) $this->get_option( 'api_url' ), (string) $this->get_option( 'api_key' ) );
			$invoice = $api->create_invoice( $request );
		} catch ( Turmite_Api_Error $e ) {
			$this->log( 'error', sprintf( 'Invoice for order %d failed: %s (HTTP %d %s)', $order->get_id(), $e->getMessage(), $e->getCode(), $e->get_error_code() ) );
			wc_add_notice( __( 'The crypto payment could not start. Try again, or select a different payment method.', 'turmite-woocommerce' ), 'error' );
			return array( 'result' => 'failure' );
		}

		$invoice_id  = (string) $invoice['id'];
		$invoice_ids = $order->get_meta( '_turmite_invoice_ids' );
		$invoice_ids = is_array( $invoice_ids ) ? $invoice_ids : array();
		if ( ! in_array( $invoice_id, $invoice_ids, true ) ) {
			$invoice_ids[] = $invoice_id;
		}
		$order->update_meta_data( '_turmite_invoice_id', $invoice_id );
		$order->update_meta_data( '_turmite_invoice_ids', $invoice_ids );
		$order->update_meta_data( '_turmite_invoice_url', $invoice['invoice_url'] );
		$order->save();
		/* translators: %s: Turmite invoice id */
		$order->add_order_note( sprintf( __( 'Turmite invoice %s created.', 'turmite-woocommerce' ), $invoice_id ) );
		$this->log( 'info', sprintf( 'Invoice %s created for order %d.', $invoice_id, $order->get_id() ) );

		return array(
			'result'   => 'success',
			'redirect' => $invoice['invoice_url'],
		);
	}

	public function thankyou_page( $order_id ) {
		$order = wc_get_order( $order_id );
		if ( $order instanceof WC_Order && $order->has_status( array( 'pending', 'on-hold' ) ) ) {
			echo '<p>' . esc_html__( 'The payment is not confirmed yet. The order status changes when the network confirms the payment.', 'turmite-woocommerce' ) . '</p>';
		}
	}

	/**
	 * Logs errors always and other levels only when the debug log is on.
	 */
	public function log( string $level, string $message ): void {
		if ( 'error' !== $level && 'yes' !== $this->get_option( 'debug' ) ) {
			return;
		}
		wc_get_logger()->log( $level, $message, array( 'source' => 'turmite' ) );
	}

	/**
	 * Filter for woocommerce_payment_complete_order_status.
	 */
	public static function filter_paid_status( $status, $order_id, $order = null ) {
		$order = $order instanceof WC_Order ? $order : wc_get_order( $order_id );
		if ( ! $order instanceof WC_Order || 'turmite' !== $order->get_payment_method() ) {
			return $status;
		}
		$settings = get_option( 'woocommerce_turmite_settings', array() );
		$wanted   = is_array( $settings ) ? (string) ( $settings['paid_status'] ?? '' ) : '';
		return in_array( $wanted, self::PAID_STATUSES, true ) ? $wanted : $status;
	}
}
