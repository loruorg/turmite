<?php
/**
 * Signature tests without PHPUnit. Run: php tests/signature-test.php
 * The fixtures come from the signing code of the Turmite server.
 *
 * @package Turmite_WooCommerce
 */

declare(strict_types=1);

require __DIR__ . '/../includes/class-turmite-signature.php';

$failures = 0;

function check( string $name, bool $ok ): void {
	global $failures;
	echo ( $ok ? 'ok    ' : 'FAIL  ' ) . $name . PHP_EOL;
	if ( ! $ok ) {
		++$failures;
	}
}

function canonical_or_error( string $raw ): string {
	try {
		return Turmite_Signature::canonical_json( $raw );
	} catch ( Throwable $e ) {
		return 'error';
	}
}

/** Changes the first digit outside a string. */
function tamper_number( string $body ): string {
	$in_string = false;
	$len       = strlen( $body );
	for ( $i = 0; $i < $len; $i++ ) {
		$c = $body[ $i ];
		if ( $in_string ) {
			if ( '\\' === $c ) {
				++$i;
			} elseif ( '"' === $c ) {
				$in_string = false;
			}
			continue;
		}
		if ( '"' === $c ) {
			$in_string = true;
		} elseif ( ctype_digit( $c ) ) {
			$body[ $i ] = (string) ( ( (int) $c + 1 ) % 10 );
			return $body;
		}
	}
	throw new RuntimeException( 'The body has no number.' );
}

$cases = json_decode( (string) file_get_contents( __DIR__ . '/fixtures/signatures.json' ), true, 512, JSON_THROW_ON_ERROR );

foreach ( $cases as $case ) {
	$name   = $case['name'];
	$secret = $case['secret'];
	$sig    = $case['sig'];
	$body   = $case['body'];

	foreach ( array( 'body', 'body_reversed', 'body_pretty' ) as $field ) {
		check( "$name: ipn signature on $field", Turmite_Signature::verify_ipn( $case[ $field ], $sig, $secret ) );
	}
	check( "$name: canonical form equals the compact server body", Turmite_Signature::canonical_json( $case['body_reversed'] ) === $body );
	check( "$name: ipn signature in upper-case hex", Turmite_Signature::verify_ipn( $body, strtoupper( $sig ), $secret ) );
	check( "$name: ipn fails with a wrong secret", ! Turmite_Signature::verify_ipn( $body, $sig, $secret . 'x' ) );
	check( "$name: ipn fails with a changed number", ! Turmite_Signature::verify_ipn( tamper_number( $body ), $sig, $secret ) );
	check( "$name: ipn fails with an empty signature", ! Turmite_Signature::verify_ipn( $body, '', $secret ) );

	$ts = (string) $case['timestamp'];
	$v1 = $case['signature_v1'];
	check( "$name: v1 signature", Turmite_Signature::verify_v1( $body, $ts, $v1, $secret, 300, (int) $ts ) );
	check( "$name: v1 signature 300 s late", Turmite_Signature::verify_v1( $body, $ts, $v1, $secret, 300, (int) $ts + 300 ) );
	check( "$name: v1 signature among two values", Turmite_Signature::verify_v1( $body, $ts, 'v1=' . str_repeat( '0', 64 ) . ', ' . $v1, $secret, 300, (int) $ts ) );
	check( "$name: v1 fails 301 s late", ! Turmite_Signature::verify_v1( $body, $ts, $v1, $secret, 300, (int) $ts + 301 ) );
	check( "$name: v1 fails with a wrong signature", ! Turmite_Signature::verify_v1( $body, $ts, 'v1=' . str_repeat( '0', 64 ), $secret, 300, (int) $ts ) );
	check( "$name: v1 fails with another timestamp", ! Turmite_Signature::verify_v1( $body, (string) ( (int) $ts + 1 ), $v1, $secret, 300, (int) $ts ) );
	check( "$name: v1 fails on the pretty body", ! Turmite_Signature::verify_v1( $case['body_pretty'], $ts, $v1, $secret, 300, (int) $ts ) );
}

$units = array(
	'key order is byte order'           => array( "{\"a\":1,\"B\":2,\"\":3,\"\u{e9}\":4,\"\u{1F600}\":5,\"\u{FF61}\":6}", "{\"\":3,\"B\":2,\"a\":1,\"\u{e9}\":4,\"\u{FF61}\":6,\"\u{1F600}\":5}" ),
	'number lexemes stay'               => array( '{"b":1.0,"a":1e+16}', '{"a":1e+16,"b":1.0}' ),
	'numeric-string keys stay strings'  => array( '{"10":1,"9":2}', '{"10":1,"9":2}' ),
	'white space goes'                  => array( " { \"a\" : [ 1 , 2 , { } ] , \"b\" :\n null }\n", '{"a":[1,2,{}],"b":null}' ),
	'escapes are normalized'            => array( '{"s":"\u00e9\/\u0001\u001F\""}', "{\"s\":\"\u{e9}/\\u0001\\u001f\\\"\"}" ),
	'nested objects in arrays sort'     => array( '[{"z":1,"y":[{"b":true,"a":false}]}]', '[{"y":[{"a":false,"b":true}],"z":1}]' ),
	'missing value is an error'         => array( '{"a":}', 'error' ),
	'trailing data is an error'         => array( '{"a":1} x', 'error' ),
	'trailing comma is an error'        => array( '[1,]', 'error' ),
	'leading zero is an error'          => array( '[01]', 'error' ),
	'raw control character is an error' => array( "[\"a\x01\"]", 'error' ),
	'bad UTF-8 is an error'             => array( "[\"\xC3\x28\"]", 'error' ),
	'lone surrogate is an error'        => array( '["\ud800"]', 'error' ),
);
foreach ( $units as $name => [ $in, $want ] ) {
	check( "canonical_json: $name", canonical_or_error( $in ) === $want );
}

echo PHP_EOL . ( 0 === $failures ? 'All checks passed.' : "$failures check(s) failed." ) . PHP_EOL;
exit( 0 === $failures ? 0 : 1 );
