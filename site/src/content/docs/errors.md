---
title: Errors
description: The error format of the merchant API, the error codes, and when to retry a request.
sidebarTitle: Errors
---

The merchant API returns errors as JSON with an HTTP status code. The format is the format of the common hosted-gateway API, so existing error handling works.

## Format

```json
{
  "status": false,
  "statusCode": 400,
  "code": "INVALID_REQUEST_PARAMS",
  "message": "`price_amount` must be greater than 0."
}
```

| Field | Description |
|---|---|
| `status` | Always `false` for an error |
| `statusCode` | The HTTP status code |
| `code` | A stable code. Use it in your code |
| `message` | A text for people. It can change. Do not parse it |

## Codes

| HTTP | `code` | Cause |
|---|---|---|
| 400 | `INVALID_REQUEST_PARAMS` | A field is missing or not valid, the body is not valid JSON, the ticker is not enabled for the project, the amount is below the minimum, or the two-factor code is wrong. The `message` tells which |
| 401 | `AUTH_REQUIRED` | The API key is missing or wrong, the payout token is missing, wrong or expired, or the email or password for `POST /v1/auth` is wrong |
| 403 | `FORBIDDEN` | The user of the payout token is not a member of the key's project, is not a project admin, or has no two-factor authentication on a mainnet install |
| 404 | `NOT_FOUND` | The path does not exist, or the payment, invoice or payout does not exist or belongs to another project |
| 405 | `METHOD_NOT_ALLOWED` | The path exists, but not with this HTTP method |
| 409 | `CONFLICT` | The request conflicts with an existing object, for example a `unique_external_id` that is used with other fields |
| 429 | `TOO_MANY_REQUESTS` | Too many requests or too many wrong codes. Wait and retry |
| 500 | `INTERNAL_ERROR` | An error on the server. The message is `Internal error.` The server log has the details |
| 503 | `UNAVAILABLE` | A service that the request needs is not available: the exchange rates, a free deposit address or a node of the network |

Treat `code` as an open set. Later versions can add values. Handle an unknown code by its HTTP status.

## Rate limits

| Limit | Value |
|---|---|
| Merchant API, per client IP address | 600 requests per minute |
| Checkout pages, per client IP address | 300 requests per minute |
| `POST /v1/auth`, per IP address and per email address | 10 attempts per minute |
| Wrong two-factor codes in `POST /v1/payout/{batch_id}/verify`, per user | 5, then the user is locked for 15 minutes |

A request above a limit gets `429`. Turmite sends no `Retry-After` header with a `429`.

## When to retry

| Response | Action |
|---|---|
| `2xx` | Done |
| `400`, `401`, `403`, `404`, `405`, `409` | Do not retry. Fix the request or the configuration |
| `429` | Retry after a delay, for example 10 s |
| `503` with `Retry-After: 30` | Retry after 30 s. There was no free deposit address, or the network has no connected node |
| `500`, `502`, `503`, `504` | Retry with backoff, for example after 1 s, 5 s and 30 s |
| Timeout or connection error | Read the state before you retry. See below |

### Timeouts on POST requests

A timeout does not tell you if Turmite created the object.

- `POST /v1/payment`: send `idempotency_key` with the request. Send the same request again after a timeout. Turmite returns the first payment.
- `POST /v1/invoice`: you can create a second invoice for the same order. The customer pays one of them. The unpaid one expires. Use the newest `invoice_url`.
- `POST /v1/payout`: give each withdrawal a `unique_external_id`. Send the same request again. Turmite returns the existing payout. Do not create a payout without one. See [Payouts](/docs/payouts/#create-a-payout-with-the-api).

## Log errors safely

- Log `statusCode`, `code` and `message`.
- Do not log the `x-api-key` header, the payout token or the request body of `POST /v1/auth`.

## Errors on your webhook endpoint

Turmite treats every response other than `2xx` from your endpoint as a failure and retries. See [Webhooks](/docs/webhooks/#retries).

Return `401` when the signature is wrong. Return `2xx` for an event that you already processed. A `5xx` response causes a retry.
