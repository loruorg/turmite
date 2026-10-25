/** A decimal amount. Requests accept a number or a decimal string. */
export type Amount = number | string;

export type PaymentStatus =
  | 'waiting'
  | 'confirming'
  | 'confirmed'
  | 'sending'
  | 'partially_paid'
  | 'finished'
  | 'failed'
  | 'refunded'
  | 'expired';

export type PayoutStatus =
  | 'pending-approval'
  | 'pending-otp-verification'
  | 'pending'
  | 'signing'
  | 'initiated'
  | 'sent'
  | 'failed'
  | 'rejected'
  | 'cancelled'
  | 'needs-review';

/** Error body of a non-2xx response. `code` is for example `INVALID_REQUEST_PARAMS`, `AUTH_REQUIRED` or `NOT_FOUND`. */
export interface ErrorBody {
  status: false;
  statusCode: number;
  code: string;
  message: string;
}

export interface StatusResponse {
  message: string;
}

export interface CurrenciesResponse {
  currencies: string[];
}

export interface FullCurrency {
  id: number;
  /** The ticker in upper case, such as `USDTTRC20`. */
  code: string;
  name: string;
  /** True when the key's project takes this ticker. Without a key: when at least one project takes it. */
  enable: boolean;
  wallet_regex: null;
  /** Sort order. */
  priority: number;
  /** True when payments on this network use a memo. */
  extra_id_exists: boolean;
  extra_id_regex: null;
  logo_url: null;
  track: false;
  cg_id: string | null;
  is_maxlimit: false;
  /** Lower-case network code, such as `tron`. */
  network: string;
  /** Token contract, or null for a native coin. */
  smart_contract: string | null;
  /** Decimals of the asset as numeric text, such as `"6"`. */
  network_precision: string;
  /** Ticker to use as `pay_currency`, such as `usdttrc20`. */
  ticker: string;
}

export interface FullCurrenciesResponse {
  currencies: FullCurrency[];
}

export interface MerchantCoinsResponse {
  selectedCurrencies: string[];
}

export interface MinAmountParams {
  /** The ticker the customer pays with. */
  currency_from: string;
  /** Copied to the response. Default `currency_from`. */
  currency_to?: string;
  /** A fiat code. Adds `fiat_equivalent` to the response when a rate is available. */
  fiat_equivalent?: string;
}

export interface MinAmountResponse {
  currency_from: string;
  currency_to: string;
  /** Smallest payment in `currency_from`. */
  min_amount: number;
  fiat_equivalent?: number;
}

export interface EstimateParams {
  amount: Amount;
  /** The price currency, such as `usd`. */
  currency_from: string;
  /** The ticker the customer pays with. */
  currency_to: string;
}

export interface EstimateResponse {
  currency_from: string;
  amount_from: number;
  currency_to: string;
  /** Decimal string. */
  estimated_amount: string;
}

/** Fields that every payment object has. Amounts are JSON numbers. */
export interface PaymentFields {
  invoice_id: number | null;
  payment_status: PaymentStatus;
  pay_address: string;
  payin_extra_id: string | null;
  price_amount: number;
  price_currency: string;
  pay_amount: number;
  actually_paid: number;
  pay_currency: string;
  order_id: string | null;
  order_description: string | null;
  outcome_amount: number | null;
  outcome_currency: string;
  /** Lower-case network code: `btc`, `xmr`, `eth`, `base`, `polygon`, `bsc`, `tron`, `sol` or `ton`. */
  network: string;
  /** Decimals of the pay currency. */
  network_precision: number;
  /** Token contract, or null for a native coin. */
  smart_contract: string | null;
  /** Latest deposit transaction. */
  payin_hash: string | null;
  /** Latest transaction that moved the funds to your wallet. */
  payout_hash: string | null;
  /** Hosted payment page. */
  payment_url: string;
  created_at: string;
  updated_at: string;
  expiration_estimate_date: string;
  /** Always null. */
  parent_payment_id: null;
  type: 'crypto2crypto';
  /** Always null. */
  time_limit: null;
  /** Always null. */
  burning_percent: null;
  /** Always null. */
  payment_extra_ids: null;
  /** `outcome_amount` after the first credit, else `pay_amount`. */
  amount_received: number;
  /** `actually_paid` in `price_currency` at the payment's rate. */
  actually_paid_at_fiat: number;
  /** Turmite takes no fee. `withdrawalFee` is the network fee of the sweeps to your wallet. */
  fee: PaymentFee;
}

export interface PaymentFee {
  /** Same as `pay_currency`. */
  currency: string;
  depositFee: number;
  withdrawalFee: number;
  serviceFee: number;
}

/** A payment from `getPayment()` and `listPayments()`. */
export interface Payment extends PaymentFields {
  payment_id: number;
  /** A number when it is a plain decimal integer, else a string. */
  purchase_id: number | string;
}

/** A payment from `createPayment()` and `createInvoicePayment()`. Both ids are strings. */
export interface CreatedPayment extends PaymentFields {
  /** Numeric text. */
  payment_id: string;
  purchase_id: string;
}

/** A payment in a webhook body. */
export interface PaymentWebhook extends PaymentFields {
  payment_id: number;
  purchase_id: string;
}

export interface CreatePaymentRequest {
  price_amount: Amount;
  price_currency: string;
  pay_currency: string;
  /** Fixed amount in `pay_currency`. Without it, the server converts `price_amount` at the current rate. */
  pay_amount?: Amount;
  order_id?: string;
  order_description?: string;
  ipn_callback_url?: string;
  success_url?: string;
  cancel_url?: string;
  purchase_id?: string;
  /** A retry with the same key returns the first payment. */
  idempotency_key?: string;
}

export type PaymentSortField =
  | 'payment_id'
  | 'payment_status'
  | 'pay_address'
  | 'price_amount'
  | 'price_currency'
  | 'pay_amount'
  | 'actually_paid'
  | 'pay_currency'
  | 'order_id'
  | 'order_description'
  | 'purchase_id'
  | 'outcome_amount'
  | 'outcome_currency'
  | 'created_at'
  | 'updated_at';

export interface ListPaymentsParams {
  /** 1 to 500. Default 10. */
  limit?: number;
  /** First page is 0. */
  page?: number;
  sortBy?: PaymentSortField;
  orderBy?: 'asc' | 'desc';
  /** Date or date-time. */
  dateFrom?: string;
  dateTo?: string;
  /** Only the payments of this invoice. */
  invoiceId?: number | string;
  payment_status?: PaymentStatus;
  order_id?: string;
}

export interface PaymentList {
  data: Payment[];
  limit: number;
  page: number;
  pagesCount: number;
  total: number;
}

export interface CreateInvoiceRequest {
  price_amount: Amount;
  price_currency: string;
  pay_currency?: string;
  order_id?: string;
  order_description?: string;
  ipn_callback_url?: string;
  success_url?: string;
  cancel_url?: string;
}

export interface Invoice {
  /** Numeric text. Use it as `iid`. */
  id: string;
  token_id: string;
  order_id: string | null;
  order_description: string | null;
  /** Decimal string. */
  price_amount: string;
  price_currency: string;
  pay_currency: string | null;
  ipn_callback_url: string | null;
  /** Send the customer here. */
  invoice_url: string;
  success_url: string | null;
  cancel_url: string | null;
  created_at: string;
  updated_at: string;
  expiration_estimate_date: string;
}

export interface CreateInvoicePaymentRequest {
  /** Invoice id. */
  iid: number | string;
  pay_currency: string;
  purchase_id?: string;
}

/** Amounts per ticker. */
export type Balance = Record<string, { amount: number; pendingAmount: number }>;

export interface AuthResponse {
  /** Bearer token for payout requests. Valid for 5 minutes. */
  token: string;
}

export interface Withdrawal {
  address: string;
  currency: string;
  amount: Amount;
  extra_id?: string;
  ipn_callback_url?: string;
  payout_description?: string;
  /** Idempotency key for this withdrawal. */
  unique_external_id?: string;
}

export interface CreatePayoutRequest {
  withdrawals: Withdrawal[];
  /** Payout webhooks for each withdrawal without its own `ipn_callback_url`. */
  ipn_callback_url?: string;
}

export interface Payout {
  id: string;
  batch_withdrawal_id: string;
  address: string;
  extra_id: string | null;
  currency: string;
  /** Network code in lower case, for example `eth` or `tron`. */
  network: string;
  /** Decimal string. */
  amount: string;
  /** Network fee in `fee_currency`, decimal string. The hot wallet pays it. */
  fee: string | null;
  /** Native coin of the network, for example `eth` or `trx`. */
  fee_currency: string;
  status: PayoutStatus;
  /** Transaction hash. */
  hash: string | null;
  error: string | null;
  ipn_callback_url: string | null;
  payout_description: string | null;
  unique_external_id: string | null;
  created_at: string;
  requested_at: string | null;
  updated_at: string;
}

export interface PayoutBatch {
  id: string;
  withdrawals: Payout[];
}

export interface ListPayoutsParams {
  batch_id?: string;
  status?: PayoutStatus;
  date_from?: string;
  date_to?: string;
  order?: 'asc' | 'desc';
  limit?: number;
  page?: number;
}

export interface PayoutList {
  payouts: Payout[];
  limit: number;
  page: number;
  pagesCount: number;
  total: number;
}

export interface ValidateAddressRequest {
  address: string;
  currency: string;
}

/** A valid address returns `"OK"`; an invalid one is a 400 error. */
export type ValidateAddressResponse = 'OK';

export interface PayoutFeeParams {
  currency: string;
  amount?: Amount;
}

export interface PayoutFee {
  currency: string;
  amount: string | null;
  /** Network fee in `currency`. */
  fee: number;
  /** Network fee in `fee_currency`. */
  network_fee: number;
  fee_currency: string;
}

interface EventBase {
  /** Same as the `x-turmite-delivery` header. */
  delivery_id: string;
  /** Unix seconds. */
  created_at: number;
}

/** Body sent to a project webhook endpoint. */
export type WebhookEvent =
  | (EventBase & { event: `payment.${PaymentStatus}`; data: PaymentWebhook })
  | (EventBase & { event: `payout.${PayoutStatus}`; data: Payout })
  | (EventBase & { event: 'test'; data: { message: string } });

/** Any webhook body: a flat payment, a flat payout, or a project endpoint event. */
export type WebhookPayload = PaymentWebhook | Payout | WebhookEvent;
