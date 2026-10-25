/**
 * A failed API request. `status` is the HTTP status, or 0 when no response came.
 * `code` is the API error code (such as `INVALID_REQUEST_PARAMS`), or one of `NETWORK_ERROR`,
 * `TIMEOUT`, `TOKEN_REQUIRED`, `HTTP_<status>`.
 */
export class TurmiteError extends Error {
  override readonly name = 'TurmiteError';
  readonly status: number;
  readonly code: string;
  /** The parsed response body, when there is one. */
  readonly body: unknown;

  constructor(status: number, code: string, message: string, options: { body?: unknown; cause?: unknown } = {}) {
    super(message, options.cause === undefined ? undefined : { cause: options.cause });
    this.status = status;
    this.code = code;
    this.body = options.body;
  }
}

export type WebhookErrorCode =
  | 'missing_header'
  | 'invalid_timestamp'
  | 'timestamp_out_of_range'
  | 'invalid_signature'
  | 'invalid_body';

/** A webhook that failed verification. Reply with a 4xx status. */
export class WebhookVerificationError extends Error {
  override readonly name = 'WebhookVerificationError';
  readonly code: WebhookErrorCode;

  constructor(code: WebhookErrorCode, message: string) {
    super(message);
    this.code = code;
  }
}
