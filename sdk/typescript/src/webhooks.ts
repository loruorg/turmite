import { bodyText, canonicalJson, isRawBody, type RawBody } from './canonical.js';
import { WebhookVerificationError } from './errors.js';
import type { PaymentStatus, WebhookPayload } from './types.js';

export const TIMESTAMP_HEADER = 'x-turmite-timestamp';
export const SIGNATURE_HEADER = 'x-turmite-signature';
/** Default name of the key-sorted HMAC-SHA512 header. Each project can change it. */
export const IPN_SIGNATURE_HEADER = 'x-turmite-sig';

/** A `Headers` object, or a plain object such as Node's `req.headers`. */
export type HeadersLike =
  | { get(name: string): string | null }
  | Record<string, string | string[] | undefined>;

export interface VerifyWebhookOptions {
  /** Largest accepted clock difference. Default 300. */
  toleranceSeconds?: number;
  /** Current time in milliseconds. For tests. */
  now?: number;
}

const encoder = new TextEncoder();

function toBytes(raw: RawBody): Uint8Array<ArrayBuffer> {
  if (typeof raw === 'string') return encoder.encode(raw);
  const view = ArrayBuffer.isView(raw) ? new Uint8Array(raw.buffer, raw.byteOffset, raw.byteLength) : new Uint8Array(raw);
  const out = new Uint8Array(view.length);
  out.set(view);
  return out;
}

function hexToBytes(hex: string): Uint8Array | null {
  if (hex.length % 2 !== 0 || !/^[0-9a-f]*$/i.test(hex)) return null;
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}

function timingSafeEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a[i]! ^ b[i]!;
  return diff === 0;
}

async function hmac(hash: 'SHA-256' | 'SHA-512', secret: string, data: Uint8Array<ArrayBuffer>): Promise<Uint8Array> {
  if (!secret) throw new TypeError('The webhook secret is empty');
  const key = await crypto.subtle.importKey('raw', encoder.encode(secret), { name: 'HMAC', hash }, false, ['sign']);
  return new Uint8Array(await crypto.subtle.sign('HMAC', key, data));
}

function header(headers: HeadersLike, name: string): string | undefined {
  if (typeof headers.get === 'function') {
    return (headers as { get(name: string): string | null }).get(name) ?? undefined;
  }
  const record = headers as Record<string, string | string[] | undefined>;
  for (const key of Object.keys(record)) {
    if (key.toLowerCase() === name) {
      const value = record[key];
      return Array.isArray(value) ? value[0] : value;
    }
  }
  return undefined;
}

/**
 * Checks the key-sorted HMAC-SHA512 signature (the `x-turmite-sig` header, or
 * the header name set for the project). Give the raw body: see `canonicalJson`.
 */
export async function verifyIpnSignature(
  rawBodyOrObject: RawBody | object,
  signatureHex: string | null | undefined,
  secret: string,
): Promise<boolean> {
  const expected = hexToBytes((signatureHex ?? '').trim());
  if (!expected || expected.length !== 64) return false;
  let canonical: string;
  try {
    canonical = canonicalJson(rawBodyOrObject);
  } catch {
    return false;
  }
  return timingSafeEqual(await hmac('SHA-512', secret, encoder.encode(canonical)), expected);
}

/**
 * Checks `x-turmite-timestamp` and `x-turmite-signature` (`v1=` hex
 * HMAC-SHA256 of `<timestamp>.<raw body>`) and returns the parsed body.
 * Throws `WebhookVerificationError` when a check fails.
 */
export async function verifyWebhook<T = WebhookPayload>(
  rawBody: RawBody,
  headers: HeadersLike,
  secret: string,
  { toleranceSeconds = 300, now = Date.now() }: VerifyWebhookOptions = {},
): Promise<T> {
  if (!isRawBody(rawBody)) {
    throw new TypeError('Give the raw request body (string, Uint8Array or ArrayBuffer)');
  }
  const timestamp = header(headers, TIMESTAMP_HEADER)?.trim();
  const signature = header(headers, SIGNATURE_HEADER);
  if (!timestamp || !signature) {
    throw new WebhookVerificationError('missing_header', `Missing ${TIMESTAMP_HEADER} or ${SIGNATURE_HEADER}`);
  }
  if (!/^\d{1,12}$/.test(timestamp)) {
    throw new WebhookVerificationError('invalid_timestamp', `Invalid ${TIMESTAMP_HEADER}`);
  }
  if (Math.abs(now / 1000 - Number(timestamp)) > toleranceSeconds) {
    throw new WebhookVerificationError('timestamp_out_of_range', 'The webhook timestamp is too old or in the future');
  }

  const body = toBytes(rawBody);
  const prefix = encoder.encode(`${timestamp}.`);
  const signed = new Uint8Array(prefix.length + body.length);
  signed.set(prefix);
  signed.set(body, prefix.length);
  const mac = await hmac('SHA-256', secret, signed);

  let valid = false;
  for (const part of signature.split(/[\s,]+/)) {
    if (!part.startsWith('v1=')) continue;
    const candidate = hexToBytes(part.slice(3));
    if (candidate && timingSafeEqual(mac, candidate)) valid = true;
  }
  if (!valid) throw new WebhookVerificationError('invalid_signature', 'The webhook signature does not match');

  try {
    return JSON.parse(bodyText(rawBody)) as T;
  } catch {
    throw new WebhookVerificationError('invalid_body', 'The webhook body is not valid JSON');
  }
}

/** True only for `finished`. Ship the order only then. */
export function isPaid(status: PaymentStatus | string): boolean {
  return status === 'finished';
}
