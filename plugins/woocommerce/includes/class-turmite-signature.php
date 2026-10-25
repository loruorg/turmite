<?php
/**
 * Webhook signature checks. This file has no WordPress dependency, so the tests load it with plain PHP.
 *
 * @package Turmite_WooCommerce
 */

declare(strict_types=1);

final class Turmite_Signature {

	private const MAX_DEPTH = 512;

	/**
	 * The key-sorted compact JSON that the HMAC-SHA512 signature covers.
	 *
	 * Object keys are sorted as byte strings. Number tokens are copied from the raw body
	 * without change, because PHP can write a number such as `1e-6` with other text.
	 *
	 * @throws InvalidArgumentException|JsonException When the body is not valid JSON.
	 */
	public static function canonical_json( string $raw ): string {
		$pos  = 0;
		$node = self::parse_value( $raw, $pos, 0 );
		self::skip_space( $raw, $pos );
		if ( strlen( $raw ) !== $pos ) {
			throw new InvalidArgumentException( 'Unexpected data after the JSON value.' );
		}
		return self::write( $node );
	}

	/**
	 * Checks the hex HMAC-SHA512 of the key-sorted body.
	 */
	public static function verify_ipn( string $raw, string $sig_hex, string $secret ): bool {
		$sig_hex = strtolower( trim( $sig_hex ) );
		if ( '' === $secret || '' === $sig_hex ) {
			return false;
		}
		try {
			$expected = hash_hmac( 'sha512', self::canonical_json( $raw ), $secret );
		} catch ( Throwable $e ) {
			return false;
		}
		return hash_equals( $expected, $sig_hex );
	}

	/**
	 * Checks `x-turmite-signature` (`v1=` + hex HMAC-SHA256 of "<timestamp>.<raw body>").
	 * The header can hold more than one value, separated by commas. One match is sufficient.
	 */
	public static function verify_v1( string $raw, string $timestamp, string $header, string $secret, int $tolerance = 300, ?int $now = null ): bool {
		$timestamp = trim( $timestamp );
		if ( '' === $secret || 1 !== preg_match( '/^\d{1,12}$/', $timestamp ) ) {
			return false;
		}
		$now = $now ?? time();
		if ( abs( $now - (int) $timestamp ) > $tolerance ) {
			return false;
		}
		$expected = hash_hmac( 'sha256', $timestamp . '.' . $raw, $secret );
		$match    = false;
		foreach ( preg_split( '/[\s,]+/', $header, -1, PREG_SPLIT_NO_EMPTY ) ?: array() as $part ) {
			if ( str_starts_with( $part, 'v1=' ) && hash_equals( $expected, strtolower( substr( $part, 3 ) ) ) ) {
				$match = true;
			}
		}
		return $match;
	}

	private static function skip_space( string $raw, int &$pos ): void {
		$pos += strspn( $raw, " \t\n\r", $pos );
	}

	/**
	 * Node shapes: ['o', [[key, node], ...]], ['a', [node, ...]], ['s', string], ['n', lexeme], ['l', literal].
	 */
	private static function parse_value( string $raw, int &$pos, int $depth ): array {
		if ( $depth > self::MAX_DEPTH ) {
			throw new InvalidArgumentException( 'JSON is nested too deep.' );
		}
		self::skip_space( $raw, $pos );
		$char = $raw[ $pos ] ?? '';
		if ( '{' === $char ) {
			return self::parse_object( $raw, $pos, $depth + 1 );
		}
		if ( '[' === $char ) {
			return self::parse_array( $raw, $pos, $depth + 1 );
		}
		if ( '"' === $char ) {
			return array( 's', self::parse_string( $raw, $pos ) );
		}
		foreach ( array( 'true', 'false', 'null' ) as $literal ) {
			if ( substr( $raw, $pos, strlen( $literal ) ) === $literal ) {
				$pos += strlen( $literal );
				return array( 'l', $literal );
			}
		}
		if ( 1 === preg_match( '/\G-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?/', $raw, $m, 0, $pos ) ) {
			$pos += strlen( $m[0] );
			return array( 'n', $m[0] );
		}
		throw new InvalidArgumentException( sprintf( 'Invalid JSON at byte %d.', $pos ) );
	}

	private static function parse_object( string $raw, int &$pos, int $depth ): array {
		++$pos;
		$members = array();
		self::skip_space( $raw, $pos );
		if ( '}' === ( $raw[ $pos ] ?? '' ) ) {
			++$pos;
			return array( 'o', $members );
		}
		while ( true ) {
			self::skip_space( $raw, $pos );
			if ( '"' !== ( $raw[ $pos ] ?? '' ) ) {
				throw new InvalidArgumentException( sprintf( 'Expected a key at byte %d.', $pos ) );
			}
			$key = self::parse_string( $raw, $pos );
			self::skip_space( $raw, $pos );
			if ( ':' !== ( $raw[ $pos ] ?? '' ) ) {
				throw new InvalidArgumentException( sprintf( 'Expected ":" at byte %d.', $pos ) );
			}
			++$pos;
			$members[] = array( $key, self::parse_value( $raw, $pos, $depth ) );
			self::skip_space( $raw, $pos );
			$char = $raw[ $pos ] ?? '';
			++$pos;
			if ( '}' === $char ) {
				break;
			}
			if ( ',' !== $char ) {
				throw new InvalidArgumentException( sprintf( 'Expected "," or "}" at byte %d.', $pos - 1 ) );
			}
		}
		usort( $members, static fn( array $a, array $b ): int => strcmp( $a[0], $b[0] ) );
		return array( 'o', $members );
	}

	private static function parse_array( string $raw, int &$pos, int $depth ): array {
		++$pos;
		$items = array();
		self::skip_space( $raw, $pos );
		if ( ']' === ( $raw[ $pos ] ?? '' ) ) {
			++$pos;
			return array( 'a', $items );
		}
		while ( true ) {
			$items[] = self::parse_value( $raw, $pos, $depth );
			self::skip_space( $raw, $pos );
			$char = $raw[ $pos ] ?? '';
			++$pos;
			if ( ']' === $char ) {
				break;
			}
			if ( ',' !== $char ) {
				throw new InvalidArgumentException( sprintf( 'Expected "," or "]" at byte %d.', $pos - 1 ) );
			}
		}
		return array( 'a', $items );
	}

	private static function parse_string( string $raw, int &$pos ): string {
		if ( 1 !== preg_match( '/\G"(?:[^"\\\\\x00-\x1f]++|\\\\.)*+"/s', $raw, $m, 0, $pos ) ) {
			throw new InvalidArgumentException( sprintf( 'Invalid string at byte %d.', $pos ) );
		}
		$pos  += strlen( $m[0] );
		$value = json_decode( $m[0], false, 512, JSON_THROW_ON_ERROR );
		if ( ! is_string( $value ) ) {
			throw new InvalidArgumentException( 'Invalid string.' );
		}
		return $value;
	}

	private static function write( array $node ): string {
		switch ( $node[0] ) {
			case 'o':
				$parts = array();
				foreach ( $node[1] as $member ) {
					$parts[] = self::write_string( $member[0] ) . ':' . self::write( $member[1] );
				}
				return '{' . implode( ',', $parts ) . '}';
			case 'a':
				$parts = array();
				foreach ( $node[1] as $item ) {
					$parts[] = self::write( $item );
				}
				return '[' . implode( ',', $parts ) . ']';
			case 's':
				return self::write_string( $node[1] );
			default:
				return $node[1];
		}
	}

	private static function write_string( string $value ): string {
		return json_encode( $value, JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE | JSON_UNESCAPED_LINE_TERMINATORS | JSON_THROW_ON_ERROR );
	}
}
