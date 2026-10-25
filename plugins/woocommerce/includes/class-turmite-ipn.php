<?php
/**
 * Receives payment updates (IPN) from Turmite at /wp-json/turmite/v1/ipn.
 *
 * @package Turmite_WooCommerce
 */

defined( 'ABSPATH' ) || exit;

final class Turmite_Ipn {

	private const MAX_EVENTS = 100;

	public static function register_routes(): void {
		register_rest_route(
			'turmite/v1',
			'/ipn',
			array(
				'methods'             => 'POST',
				'callback'            => array( self::class, 'handle' ),
				'permission_callback' => '__return_true',
			)
		);
	}

	public static function handle( WP_REST_Request $request ): WP_REST_Response {
		$gateway = self::gateway();
		if ( null === $gateway ) {
			return self::reply( 503, 'gateway not loaded' );
		}

		$raw    = (string) $request->get_body();
		$secret = (string) $gateway->get_option( 'ipn_secret' );
		$header = (string) $gateway->get_option( 'sig_header' );
		$sig    = (string) $request->get_header( '' !== $header ? $header : 'x-turmite-sig' );
		if ( ! Turmite_Signature::verify_ipn( $raw, $sig, $secret ) ) {
			$gateway->log( 'warning', 'IPN rejected: the signature is missing or wrong.' );
			return self::reply( 401, 'bad signature' );
		}
		$timestamp = $request->get_header( 'x-turmite-timestamp' );
		$v1        = $request->get_header( 'x-turmite-signature' );
		if ( null !== $timestamp && null !== $v1 && ! Turmite_Signature::verify_v1( $raw, $timestamp, $v1, $secret ) ) {
			$gateway->log( 'warning', 'IPN rejected: the v1 signature is wrong or the timestamp is too old.' );
			return self::reply( 401, 'bad signature' );
		}

		$data = json_decode( $raw, true, 64, JSON_BIGINT_AS_STRING );
		if ( ! is_array( $data ) ) {
			return self::reply( 400, 'bad json' );
		}
		if ( isset( $data['event'], $data['data'] ) && is_array( $data['data'] ) ) {
			$data = $data['data'];
		}
		if ( ! isset( $data['payment_id'], $data['payment_status'] ) ) {
			// Test and payout events have no payment.
			return self::reply( 200 );
		}

		$order_ref = (string) ( $data['order_id'] ?? '' );
		$order     = ctype_digit( $order_ref ) ? wc_get_order( (int) $order_ref ) : false;
		if ( ! $order instanceof WC_Order ) {
			$gateway->log( 'warning', sprintf( 'IPN for an unknown order "%s".', sanitize_text_field( $order_ref ) ) );
			return self::reply( 404, 'order not found' );
		}
		if ( 'turmite' !== $order->get_payment_method() ) {
			$gateway->log( 'warning', sprintf( 'IPN ignored: order %d uses a different payment method.', $order->get_id() ) );
			return self::reply( 200 );
		}

		self::apply( $order, $data, $gateway );
		return self::reply( 200 );
	}

	private static function apply( WC_Order $order, array $data, Turmite_Gateway $gateway ): void {
		$payment_id = (string) $data['payment_id'];
		$status     = (string) $data['payment_status'];
		$paid       = self::number( $data['actually_paid'] ?? null );
		$pay_amount = self::number( $data['pay_amount'] ?? null );
		$currency   = strtoupper( (string) ( $data['pay_currency'] ?? '' ) );
		$network    = strtoupper( (string) ( $data['network'] ?? '' ) );

		// Each credit after `finished` comes again as `finished` with a larger actually_paid.
		$event  = $payment_id . ':' . $status . ( 'finished' === $status ? ':' . $paid : '' );
		$events = $order->get_meta( '_turmite_events' );
		$events = is_array( $events ) ? $events : array();
		if ( in_array( $event, $events, true ) ) {
			$gateway->log( 'info', sprintf( 'IPN %s for order %d is a repeat.', $event, $order->get_id() ) );
			return;
		}
		$events[] = $event;
		$order->update_meta_data( '_turmite_events', array_slice( $events, -self::MAX_EVENTS ) );

		$mismatch = self::mismatch( $order, $data );
		if ( '' !== $mismatch ) {
			$note = sprintf(
				/* translators: 1: payment id, 2: payment status, 3: list of fields that differ */
				__( 'Turmite payment %1$s (%2$s) does not agree with the order: %3$s. Examine the payment in the Turmite dashboard.', 'turmite-woocommerce' ),
				$payment_id,
				$status,
				$mismatch
			);
			if ( ! $order->is_paid() && ! $order->has_status( array( 'on-hold', 'refunded' ) ) ) {
				$order->update_status( 'on-hold', $note );
			} else {
				$order->add_order_note( $note );
			}
			$order->save();
			return;
		}

		$latest  = (string) $order->get_meta( '_turmite_payment_id' );
		$current = '' === $latest || self::compare_ids( $payment_id, $latest ) >= 0 || $payment_id === $order->get_transaction_id();
		if ( '' === $latest || self::compare_ids( $payment_id, $latest ) >= 0 ) {
			$order->update_meta_data( '_turmite_payment_id', $payment_id );
			$order->update_meta_data( '_turmite_payment_status', $status );
		}
		if ( ! $current && 'finished' !== $status ) {
			$gateway->log( 'info', sprintf( 'IPN %s for order %d is for an older payment. No change.', $event, $order->get_id() ) );
			$order->save();
			return;
		}

		switch ( $status ) {
			case 'finished':
				if ( $order->is_paid() ) {
					$note = $payment_id === $order->get_transaction_id()
						/* translators: 1: payment id, 2: amount, 3: currency */
						? sprintf( __( 'Turmite payment %1$s received more funds: %2$s %3$s in total.', 'turmite-woocommerce' ), $payment_id, $paid, $currency )
						/* translators: 1: payment id, 2: amount, 3: currency */
						: sprintf( __( 'The order is already paid. Turmite payment %1$s also finished with %2$s %3$s. Examine it for a double payment.', 'turmite-woocommerce' ), $payment_id, $paid, $currency );
					$order->add_order_note( $note );
					break;
				}
				$note = sprintf(
					/* translators: 1: payment id, 2: amount, 3: currency, 4: network */
					__( 'Turmite payment %1$s finished: %2$s %3$s received on %4$s.', 'turmite-woocommerce' ),
					$payment_id,
					$paid,
					$currency,
					$network
				);
				if ( ! empty( $data['payin_hash'] ) && is_string( $data['payin_hash'] ) ) {
					/* translators: %s: transaction hash */
					$note .= ' ' . sprintf( __( 'Transaction: %s.', 'turmite-woocommerce' ), $data['payin_hash'] );
				}
				$order->add_order_note( $note );
				$order->payment_complete( $payment_id );
				break;

			case 'partially_paid':
				/* translators: 1: payment id, 2: amount received, 3: amount due, 4: currency */
				$note = sprintf( __( 'Turmite payment %1$s is partially paid: %2$s of %3$s %4$s received.', 'turmite-woocommerce' ), $payment_id, $paid, $pay_amount, $currency );
				if ( $order->is_paid() || $order->has_status( array( 'on-hold', 'refunded' ) ) ) {
					$order->add_order_note( $note );
				} else {
					$order->update_status( 'on-hold', $note );
				}
				break;

			case 'expired':
			case 'failed':
				/* translators: 1: payment id, 2: payment status */
				$note = sprintf( __( 'Turmite payment %1$s: %2$s.', 'turmite-woocommerce' ), $payment_id, $status );
				if ( $order->has_status( 'pending' ) ) {
					$order->update_status( 'expired' === $status ? 'cancelled' : 'failed', $note );
				} else {
					$order->add_order_note( $note );
				}
				break;

			case 'refunded':
				/* translators: %s: payment id */
				$note = sprintf( __( 'Turmite payment %s is refunded.', 'turmite-woocommerce' ), $payment_id );
				if ( $order->is_paid() || $order->has_status( 'on-hold' ) ) {
					$order->update_status( 'refunded', $note );
				} else {
					$order->add_order_note( $note );
				}
				break;

			default:
				/* translators: 1: payment id, 2: payment status */
				$order->add_order_note( sprintf( __( 'Turmite payment %1$s: %2$s.', 'turmite-woocommerce' ), $payment_id, $status ) );
		}
		$order->save();
		$gateway->log( 'info', sprintf( 'IPN %s applied to order %d.', $event, $order->get_id() ) );
	}

	/**
	 * Names of the IPN fields that do not agree with the order, or an empty string.
	 */
	private static function mismatch( WC_Order $order, array $data ): string {
		$fields      = array();
		$invoice_ids = $order->get_meta( '_turmite_invoice_ids' );
		$invoice_ids = is_array( $invoice_ids ) ? $invoice_ids : array();
		$stored      = (string) $order->get_meta( '_turmite_invoice_id' );
		if ( '' !== $stored && ! in_array( $stored, $invoice_ids, true ) ) {
			$invoice_ids[] = $stored;
		}
		$invoice_ref = isset( $data['invoice_id'] ) ? (string) $data['invoice_id'] : '';
		if ( '' !== $invoice_ref && array() !== $invoice_ids && ! in_array( $invoice_ref, $invoice_ids, true ) ) {
			$fields[] = 'invoice_id';
		}
		if ( strtolower( (string) ( $data['price_currency'] ?? '' ) ) !== strtolower( $order->get_currency() ) ) {
			$fields[] = 'price_currency';
		}
		$price    = $data['price_amount'] ?? null;
		$decimals = wc_get_price_decimals();
		if ( ! is_numeric( $price ) || number_format( (float) $price, $decimals, '.', '' ) !== number_format( (float) $order->get_total(), $decimals, '.', '' ) ) {
			$fields[] = 'price_amount';
		}
		return implode( ', ', $fields );
	}

	/**
	 * Compares two decimal id strings without integer overflow.
	 */
	private static function compare_ids( string $a, string $b ): int {
		$a = ltrim( $a, '0' );
		$b = ltrim( $b, '0' );
		return strlen( $a ) <=> strlen( $b ) ?: strcmp( $a, $b ) <=> 0;
	}

	/**
	 * A JSON number as plain decimal text, for order notes.
	 */
	private static function number( $value ): string {
		if ( is_int( $value ) || is_string( $value ) ) {
			return (string) $value;
		}
		if ( ! is_float( $value ) ) {
			return '?';
		}
		$text = (string) json_encode( $value );
		if ( false === stripos( $text, 'e' ) ) {
			return (string) preg_replace( '/\.0$/', '', $text );
		}
		[ $mantissa, $exponent ] = explode( 'e', strtolower( $text ) );
		$fraction                = strrchr( $mantissa, '.' );
		$decimals                = max( 0, ( false === $fraction ? 0 : strlen( $fraction ) - 1 ) - (int) $exponent );
		$plain                   = number_format( $value, min( $decimals, 30 ), '.', '' );
		return str_contains( $plain, '.' ) ? rtrim( rtrim( $plain, '0' ), '.' ) : $plain;
	}

	private static function gateway(): ?Turmite_Gateway {
		if ( ! function_exists( 'WC' ) ) {
			return null;
		}
		$gateways = WC()->payment_gateways()->payment_gateways();
		$gateway  = $gateways['turmite'] ?? null;
		return $gateway instanceof Turmite_Gateway ? $gateway : null;
	}

	private static function reply( int $status, string $error = '' ): WP_REST_Response {
		$body = '' === $error ? array( 'ok' => true ) : array(
			'ok'    => false,
			'error' => $error,
		);
		return new WP_REST_Response( $body, $status );
	}
}
