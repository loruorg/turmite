<?php
/**
 * Client for the Turmite merchant API.
 *
 * @package Turmite_WooCommerce
 */

defined( 'ABSPATH' ) || exit;

final class Turmite_Api_Error extends Exception {

	public function __construct( string $message, int $status = 0, private string $error_code = '' ) {
		parent::__construct( $message, $status );
	}

	public function get_error_code(): string {
		return $this->error_code;
	}
}

final class Turmite_Api {

	private string $base_url;

	public function __construct( string $base_url, private string $api_key ) {
		$this->base_url = untrailingslashit( trim( $base_url ) );
	}

	/**
	 * POST /v1/invoice.
	 *
	 * @param array<string, string> $body Invoice fields.
	 * @return array<string, mixed> The invoice.
	 * @throws Turmite_Api_Error When the request fails or the answer is not an invoice.
	 */
	public function create_invoice( array $body ): array {
		$invoice = $this->request( 'POST', '/v1/invoice', $body );
		$url     = $invoice['invoice_url'] ?? '';
		if ( ! isset( $invoice['id'] ) || ! is_string( $url ) || ! in_array( wp_parse_url( $url, PHP_URL_SCHEME ), array( 'http', 'https' ), true ) ) {
			throw new Turmite_Api_Error( 'The answer has no invoice id or invoice URL.' );
		}
		return $invoice;
	}

	private function request( string $method, string $path, ?array $body = null ): array {
		if ( '' === $this->base_url || '' === $this->api_key ) {
			throw new Turmite_Api_Error( 'The Turmite URL or the API key is not set.' );
		}
		$args = array(
			'method'      => $method,
			'timeout'     => 20,
			'redirection' => 0,
			'user-agent'  => 'turmite-woocommerce/' . TURMITE_WC_VERSION,
			'headers'     => array(
				'x-api-key'    => $this->api_key,
				'accept'       => 'application/json',
				'content-type' => 'application/json',
			),
		);
		if ( null !== $body ) {
			$args['body'] = (string) wp_json_encode( $body );
		}
		$response = wp_remote_request( $this->base_url . $path, $args );
		if ( is_wp_error( $response ) ) {
			throw new Turmite_Api_Error( $response->get_error_message() );
		}
		$status = (int) wp_remote_retrieve_response_code( $response );
		$data   = json_decode( wp_remote_retrieve_body( $response ), true );
		if ( $status < 200 || $status > 299 ) {
			$message = is_array( $data ) && isset( $data['message'] ) ? (string) $data['message'] : 'HTTP ' . $status;
			$code    = is_array( $data ) && isset( $data['code'] ) ? (string) $data['code'] : '';
			throw new Turmite_Api_Error( $message, $status, $code );
		}
		if ( ! is_array( $data ) ) {
			throw new Turmite_Api_Error( 'The answer is not JSON.', $status );
		}
		return $data;
	}
}
