import { TurmiteError } from './errors.js';
import type {
  AuthResponse,
  Balance,
  CreateInvoicePaymentRequest,
  CreateInvoiceRequest,
  CreatePaymentRequest,
  CreatePayoutRequest,
  CreatedPayment,
  CurrenciesResponse,
  EstimateParams,
  EstimateResponse,
  FullCurrenciesResponse,
  Invoice,
  ListPaymentsParams,
  ListPayoutsParams,
  MerchantCoinsResponse,
  MinAmountParams,
  MinAmountResponse,
  Payment,
  PaymentList,
  PayoutBatch,
  PayoutFee,
  PayoutFeeParams,
  PayoutList,
  StatusResponse,
  ValidateAddressRequest,
  ValidateAddressResponse,
} from './types.js';

export type FetchLike = (input: string, init: RequestInit) => Promise<Response>;

export interface TurmiteOptions {
  /** Your Turmite site URL. It can have a path: `https://example.com/pay`. */
  baseUrl: string;
  /** Project API key (`tm_…`). */
  apiKey?: string;
  /** Replaces the global `fetch`. */
  fetch?: FetchLike;
  /** Request timeout in milliseconds. Default 30000. */
  timeoutMs?: number;
}

/** Overrides the token that `auth()` keeps. */
export interface TokenOptions {
  token?: string;
}

type Query = Record<string, string | number | undefined>;

interface RequestSpec {
  query?: Query | undefined;
  body?: unknown;
  /** Bearer token required. */
  bearer?: boolean;
  token?: string | undefined;
}

export class Turmite {
  readonly baseUrl: string;
  readonly #apiKey: string | undefined;
  readonly #fetch: FetchLike;
  readonly #timeoutMs: number;
  #token: string | undefined;

  constructor(options: TurmiteOptions) {
    if (!options?.baseUrl) throw new TypeError('baseUrl is required');
    this.baseUrl = options.baseUrl.replace(/\/+$/, '');
    this.#apiKey = options.apiKey;
    const f = options.fetch ?? (globalThis.fetch as FetchLike | undefined);
    if (!f) throw new TypeError('No fetch available. Pass the fetch option.');
    this.#fetch = (input, init) => f(input, init);
    this.#timeoutMs = options.timeoutMs ?? 30_000;
  }

  async #request<T>(method: string, path: string, spec: RequestSpec = {}): Promise<T> {
    let url = this.baseUrl + path;
    if (spec.query) {
      const params = new URLSearchParams();
      for (const [key, value] of Object.entries(spec.query)) {
        if (value !== undefined) params.set(key, String(value));
      }
      const qs = params.toString();
      if (qs) url += `?${qs}`;
    }

    const headers: Record<string, string> = { accept: 'application/json' };
    if (this.#apiKey) headers['x-api-key'] = this.#apiKey;
    if (spec.bearer) {
      const token = spec.token ?? this.#token;
      if (!token) throw new TurmiteError(0, 'TOKEN_REQUIRED', 'Call auth() first or pass a token');
      headers.authorization = `Bearer ${token}`;
    }
    const init: RequestInit = { method, headers, signal: AbortSignal.timeout(this.#timeoutMs) };
    if (spec.body !== undefined) {
      headers['content-type'] = 'application/json';
      init.body = JSON.stringify(spec.body);
    }

    let res: Response;
    let text: string;
    try {
      res = await this.#fetch(url, init);
      text = await res.text();
    } catch (cause) {
      const timeout = cause instanceof Error && cause.name === 'TimeoutError';
      throw new TurmiteError(
        0,
        timeout ? 'TIMEOUT' : 'NETWORK_ERROR',
        timeout ? `No response in ${this.#timeoutMs} ms` : `Request failed: ${String(cause)}`,
        { cause },
      );
    }

    let body: unknown = undefined;
    if (text) {
      try {
        body = JSON.parse(text);
      } catch {
        body = text;
      }
    }
    if (!res.ok) {
      const b = (body && typeof body === 'object' ? body : {}) as Record<string, unknown>;
      const code = typeof b.code === 'string' ? b.code : `HTTP_${res.status}`;
      const message =
        typeof b.message === 'string'
          ? b.message
          : typeof body === 'string' && body
            ? body
            : `${res.status} ${res.statusText}`.trim();
      throw new TurmiteError(res.status, code, message, { body });
    }
    return body as T;
  }

  /** `GET /v1/status` */
  status(): Promise<StatusResponse> {
    return this.#request('GET', '/v1/status');
  }

  /** `GET /v1/currencies`: tickers enabled for the project. */
  currencies(): Promise<CurrenciesResponse> {
    return this.#request('GET', '/v1/currencies');
  }

  /** `GET /v1/full-currencies` */
  fullCurrencies(): Promise<FullCurrenciesResponse> {
    return this.#request('GET', '/v1/full-currencies');
  }

  /** `GET /v1/merchant/coins` */
  merchantCoins(): Promise<MerchantCoinsResponse> {
    return this.#request('GET', '/v1/merchant/coins');
  }

  /** `GET /v1/min-amount` */
  minAmount(params: MinAmountParams): Promise<MinAmountResponse> {
    return this.#request('GET', '/v1/min-amount', { query: { ...params } });
  }

  /** `GET /v1/estimate` */
  estimate(params: EstimateParams): Promise<EstimateResponse> {
    return this.#request('GET', '/v1/estimate', { query: { ...params } });
  }

  /** `POST /v1/payment`. In this response `payment_id` and `purchase_id` are strings. */
  createPayment(body: CreatePaymentRequest): Promise<CreatedPayment> {
    return this.#request('POST', '/v1/payment', { body });
  }

  /** `GET /v1/payment/{payment_id}` */
  getPayment(paymentId: number | string): Promise<Payment> {
    return this.#request('GET', `/v1/payment/${encodeURIComponent(paymentId)}`);
  }

  /** `GET /v1/payment` */
  listPayments(params: ListPaymentsParams = {}): Promise<PaymentList> {
    return this.#request('GET', '/v1/payment', { query: { ...params } });
  }

  /** `POST /v1/invoice`: send the customer to `invoice_url`. */
  createInvoice(body: CreateInvoiceRequest): Promise<Invoice> {
    return this.#request('POST', '/v1/invoice', { body });
  }

  /** `POST /v1/invoice-payment`: creates a payment for an invoice, or returns its open payment in that coin. */
  createInvoicePayment(body: CreateInvoicePaymentRequest): Promise<CreatedPayment> {
    return this.#request('POST', '/v1/invoice-payment', { body });
  }

  /** `GET /v1/balance` */
  balance(): Promise<Balance> {
    return this.#request('GET', '/v1/balance');
  }

  /**
   * `POST /v1/auth` with the email and password of a dashboard user.
   * The client keeps the token (valid 5 minutes) for the payout methods.
   */
  async auth(email: string, password: string): Promise<AuthResponse> {
    const res = await this.#request<AuthResponse>('POST', '/v1/auth', { body: { email, password } });
    this.#token = res.token;
    return res;
  }

  /** `POST /v1/payout`. Needs `auth()` first. */
  createPayout(body: CreatePayoutRequest, options: TokenOptions = {}): Promise<PayoutBatch> {
    return this.#request('POST', '/v1/payout', { body, bearer: true, token: options.token });
  }

  /** `POST /v1/payout/{batch_id}/verify` with the user's current TOTP code. */
  verifyPayout(batchId: string, verificationCode: string, options: TokenOptions = {}): Promise<PayoutBatch> {
    return this.#request('POST', `/v1/payout/${encodeURIComponent(batchId)}/verify`, {
      body: { verification_code: verificationCode },
      bearer: true,
      token: options.token,
    });
  }

  /** `GET /v1/payout/{batch_id}` */
  getPayout(batchId: string): Promise<PayoutBatch> {
    return this.#request('GET', `/v1/payout/${encodeURIComponent(batchId)}`);
  }

  /** `GET /v1/payout` */
  listPayouts(params: ListPayoutsParams = {}, options: TokenOptions = {}): Promise<PayoutList> {
    return this.#request('GET', '/v1/payout', { query: { ...params }, bearer: true, token: options.token });
  }

  /** `POST /v1/payout/validate-address` */
  validateAddress(body: ValidateAddressRequest): Promise<ValidateAddressResponse> {
    return this.#request('POST', '/v1/payout/validate-address', { body });
  }

  /** `GET /v1/payout/fee` */
  payoutFee(params: PayoutFeeParams): Promise<PayoutFee> {
    return this.#request('GET', '/v1/payout/fee', { query: { ...params } });
  }
}
