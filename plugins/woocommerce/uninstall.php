<?php
/**
 * Removes the plugin settings.
 *
 * @package Turmite_WooCommerce
 */

defined( 'WP_UNINSTALL_PLUGIN' ) || exit;

delete_option( 'woocommerce_turmite_settings' );
