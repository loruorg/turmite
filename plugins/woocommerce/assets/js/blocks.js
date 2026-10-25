( function () {
	const { registerPaymentMethod } = window.wc.wcBlocksRegistry;
	const { getSetting } = window.wc.wcSettings;
	const { createElement } = window.wp.element;
	const { decodeEntities } = window.wp.htmlEntities;
	const { __ } = window.wp.i18n;

	const settings = getSetting( 'turmite_data', {} );
	const title = decodeEntities( settings.title || '' ) || __( 'Crypto', 'turmite-woocommerce' );
	const description = decodeEntities( settings.description || '' );

	const Content = () => createElement( 'div', null, description );
	const Label = ( props ) => {
		const { PaymentMethodLabel } = props.components;
		return createElement( PaymentMethodLabel, { text: title } );
	};

	registerPaymentMethod( {
		name: 'turmite',
		label: createElement( Label ),
		content: createElement( Content ),
		edit: createElement( Content ),
		canMakePayment: () => true,
		ariaLabel: title,
		placeOrderButtonLabel: __( 'Continue to payment', 'turmite-woocommerce' ),
		supports: { features: settings.supports || [ 'products' ] },
	} );
} )();
