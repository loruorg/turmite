---
title: Webhooks
description: Receive signed HTTP requests when a payment or a payout changes status, and verify them.
sidebarTitle: Webhooks
---

Turmite sends an HTTP `POST` to your server when a payment or a payout changes status. Each request is signed. Verify the signature before you use the body.

## Kinds of webhook

| Kind | Where you set it | Body |
|---|---|---|
| Per-payment IPN | `ipn_callback_url` in `POST /v1/payment` or `POST /v1/invoice` | The flat [payment object](/docs/merchant-api/#the-payment-object) |
| Per-payout IPN | `ipn_callback_url` of a withdrawal or of the batch in `POST /v1/payout` | The flat [payout object](/docs/payouts/#the-payout-object) |
| Project endpoint | Dashboard → **Webhooks** → **Endpoint URL** → **Add endpoint** | An envelope with `event`, `delivery_id`, `created_at` and `data` |

The per-payment IPN works with code written for the common hosted-gateway API. A payment made from an invoice sends its IPN to the invoice's `ipn_callback_url`. Project endpoints receive payment events, payout events and test events. A user with the `developer` role can add and delete endpoints.

Turmite sends a payment event at each status change, also when a payment returns to an earlier status. After `finished`, a later deposit sends the `finished` event again with the new `actually_paid`, and its credit sends it with the new `outcome_amount`, for example for a late top-up. The per-payment IPN also gets an event at each new confirmation count while the status is `confirming`.

Example body for a per-payment IPN:

```json
{
  "actually_paid": 0.00041235,
  "created_at": "2026-10-23T12:00:00.000Z",
  "expiration_estimate_date": "2026-10-23T13:00:00.000Z",
  "invoice_id": null,
  "network": "btc",
  "network_precision": 8,
  "order_description": "Order 1001",
  "order_id": "ORDER-1001",
  "outcome_amount": 0.00041235,
  "outcome_currency": "btc",
  "pay_address": "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
  "pay_amount": 0.00041235,
  "pay_currency": "btc",
  "payin_extra_id": null,
  "payin_hash": "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b",
  "payment_id": 5524759814,
  "payment_status": "finished",
  "payment_url": "https://pay.example.com/pay/Q7mX2pK9vT4nB8cR1sW6yZ3h",
  "payout_hash": null,
  "price_amount": 25,
  "price_currency": "usd",
  "purchase_id": "4709931082",
  "smart_contract": null,
  "updated_at": "2026-10-23T12:41:07.000Z"
}
```

Example body for a project endpoint:

```json
{
  "event": "payment.finished",
  "delivery_id": "dK4rT8wQ2mN6xP1vB9cL5zH7",
  "created_at": 1792759268,
  "data": {
    "payment_id": 5524759814,
    "payment_status": "finished",
    "order_id": "ORDER-1001"
  }
}
```

`data` holds the full payment object or payout object. The example shows three fields only. A `test` event has `{"message": "Test event from Turmite."}` in `data`. `delivery_id` is a random token of 24 letters and digits, the same value as the `x-turmite-delivery` header. `created_at` is Unix time in seconds.

In both bodies, `payment_id` is a number and `purchase_id` is a string.

## Events

| Event | Sent when |
|---|---|
| `payment.<status>` | A payment gets a new status, for example `payment.confirming` or `payment.finished` |
| `payout.<status>` | A payout gets a new status, for example `payout.sent` or `payout.needs-review` |
| `test` | You click **Send test** in the dashboard |

The payment statuses are in [Payment statuses](/docs/statuses/). The payout statuses are in [Payouts](/docs/payouts/#statuses).

## Headers

Every delivery has these headers.

| Header | Value |
|---|---|
| `content-type` | `application/json` |
| Signature header, default `x-turmite-sig` | Hex HMAC-SHA512 of the key-sorted JSON body. Key: the secret of the webhook. See [The secrets](#the-secrets) |
| `x-turmite-timestamp` | Unix time in seconds when Turmite signed this attempt |
| `x-turmite-signature` | `v1=` and the hex HMAC-SHA256 of `<timestamp>.<raw body>`. Key: the same secret |
| `x-turmite-event` | Event name, for example `payment.finished` |
| `x-turmite-delivery` | Delivery ID. Retries of one delivery use the same ID |
| `user-agent` | `turmite-webhook/1` |

A project admin can change the name of the signature header in **Settings** → **General** → **IPN signature header**. Set it to the header name that your existing integration reads. The value does not change. The name can have up to 64 letters, digits and hyphens. Turmite stores it in lowercase. It cannot start with `x-turmite-`, except `x-turmite-sig`.

## The secrets

Both signatures of one delivery use the same secret. Which secret depends on the kind of webhook.

| Webhook | Secret | Where to get it |
|---|---|---|
| `ipn_callback_url` of a payment, an invoice or a withdrawal | The project IPN secret | **Settings** → **IPN secret**. Enter a fresh two-factor code and click **Show secret** |
| Project endpoint | The signing secret of that endpoint | Shown one time under **Signing secret**, when you add the endpoint |

To change the IPN secret, a project admin clicks **Rotate** under **IPN secret** and enters a fresh two-factor code. To change an endpoint secret, add a new endpoint, switch your receiver to it, then delete the old endpoint.

Each secret is 48 hex characters. Turmite stores each secret sealed with `TURMITE_SECRET`. Keep the secrets on your server, with the API key.

## Which signature to use

- For new code, verify `x-turmite-signature`. It covers a timestamp, so you can refuse replayed requests.
- For an existing integration, keep the signature check that it has. Set the signature header name in **Settings** if your code reads another name.

You can check both.

## Verify the timestamp signature

1. Read the raw body bytes before you parse JSON.
2. Read `x-turmite-timestamp`. Refuse the request if it is more than 5 minutes from your clock.
3. Calculate HMAC-SHA256 with the secret over the timestamp, a full stop and the raw body.
4. Encode the result as lowercase hex and add `v1=` before it.
5. Compare the result with `x-turmite-signature` in constant time.

The examples use the IPN secret in `TM_IPN_SECRET`. For a project endpoint, use the secret of the endpoint.

### Node.js

```js
import crypto from 'node:crypto';
import express from 'express';

const secret = process.env.TM_IPN_SECRET;

function verifyV1(rawBody, headers) {
  const ts = String(headers['x-turmite-timestamp'] ?? '');
  const sig = String(headers['x-turmite-signature'] ?? '');
  if (!Buffer.isBuffer(rawBody)) return false;
  if (!/^\d+$/.test(ts) || Math.abs(Date.now() / 1000 - Number(ts)) > 300) return false;
  const mac = crypto.createHmac('sha256', secret).update(`${ts}.`).update(rawBody).digest('hex');
  const want = Buffer.from(`v1=${mac}`);
  const got = Buffer.from(sig);
  return want.length === got.length && crypto.timingSafeEqual(want, got);
}

const app = express();
app.post('/turmite/webhook', express.raw({ type: 'application/json' }), (req, res) => {
  if (!verifyV1(req.body, req.headers)) return res.sendStatus(401);
  const event = JSON.parse(req.body.toString('utf8'));
  // Update the order here. Make this step idempotent.
  res.sendStatus(200);
});
app.listen(3000);
```

### PHP

```php
<?php
$secret = getenv('TM_IPN_SECRET');
$raw = file_get_contents('php://input');
$ts = $_SERVER['HTTP_X_TURMITE_TIMESTAMP'] ?? '';
$sig = $_SERVER['HTTP_X_TURMITE_SIGNATURE'] ?? '';

$want = 'v1=' . hash_hmac('sha256', $ts . '.' . $raw, $secret);
if (!ctype_digit($ts) || abs(time() - (int) $ts) > 300 || !hash_equals($want, $sig)) {
    http_response_code(401);
    exit;
}

$event = json_decode($raw, true);
// Update the order here. Make this step idempotent.
http_response_code(200);
```

### Python

```python
import hashlib
import hmac
import os
import time

from flask import Flask, abort, request

app = Flask(__name__)
SECRET = os.environ["TM_IPN_SECRET"].encode()


def verify_v1(raw: bytes, headers) -> bool:
    ts = headers.get("x-turmite-timestamp", "")
    sig = headers.get("x-turmite-signature", "")
    if not ts.isdigit() or abs(time.time() - int(ts)) > 300:
        return False
    mac = hmac.new(SECRET, ts.encode() + b"." + raw, hashlib.sha256).hexdigest()
    return hmac.compare_digest("v1=" + mac, sig)


@app.post("/turmite/webhook")
def webhook():
    raw = request.get_data()
    if not verify_v1(raw, request.headers):
        abort(401)
    event = request.get_json()
    # Update the order here. Make this step idempotent.
    return "", 200
```

### Go

```go
package main

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"io"
	"net/http"
	"os"
	"strconv"
	"time"
)

var secret = []byte(os.Getenv("TM_IPN_SECRET"))

func verifyV1(h http.Header, body []byte) bool {
	ts := h.Get("X-Turmite-Timestamp")
	n, err := strconv.ParseInt(ts, 10, 64)
	if err != nil {
		return false
	}
	if d := time.Now().Unix() - n; d > 300 || d < -300 {
		return false
	}
	mac := hmac.New(sha256.New, secret)
	mac.Write([]byte(ts + "."))
	mac.Write(body)
	want := "v1=" + hex.EncodeToString(mac.Sum(nil))
	return hmac.Equal([]byte(want), []byte(h.Get("X-Turmite-Signature")))
}

func webhook(w http.ResponseWriter, r *http.Request) {
	body, err := io.ReadAll(io.LimitReader(r.Body, 1<<20))
	if err != nil || !verifyV1(r.Header, body) {
		w.WriteHeader(http.StatusUnauthorized)
		return
	}
	// Parse body and update the order here. Make this step idempotent.
	w.WriteHeader(http.StatusOK)
}

func main() {
	http.HandleFunc("POST /turmite/webhook", webhook)
	http.ListenAndServe(":3000", nil)
}
```

### Ruby

```ruby
require "json"
require "openssl"
require "sinatra"

SECRET = ENV.fetch("TM_IPN_SECRET")

def verify_v1(raw, ts, sig)
  return false unless ts.to_s.match?(/\A\d+\z/)
  return false if (Time.now.to_i - ts.to_i).abs > 300

  want = "v1=" + OpenSSL::HMAC.hexdigest("SHA256", SECRET, "#{ts}.#{raw}")
  OpenSSL.secure_compare(want, sig.to_s)
end

post "/turmite/webhook" do
  raw = request.body.read
  ts = request.env["HTTP_X_TURMITE_TIMESTAMP"]
  sig = request.env["HTTP_X_TURMITE_SIGNATURE"]
  halt 401 unless verify_v1(raw, ts, sig)

  event = JSON.parse(raw)
  # Update the order here. Make this step idempotent.
  status 200
end
```

## Verify the compatible signature

This is the signature of the common hosted-gateway API. Existing integrations already check it.

1. Make the key-sorted JSON from the raw body:
   - Sort the members of each object by key. Compare keys as UTF-8 bytes. Keep the order of arrays.
   - Write compact JSON with no whitespace.
   - Copy each number exactly as it is in the raw body.
   - In strings, escape only `"`, `\` and control characters. Do not escape `/` or non-ASCII characters.
2. Calculate HMAC-SHA512 with the secret over that text.
3. Encode the result as lowercase hex.
4. Compare the result with the signature header in constant time.

The body is the key-sorted JSON. Turmite writes numbers as JavaScript does: a whole amount as an integer (`25`), other amounts in the shortest decimal form (`0.00041235`, `0.000001`), and an exponent only below 0.0000001 or from 1e21 (`1e-7`, `1.5e-7`, `1e+21`). In JavaScript, `JSON.stringify` of the parsed body with sorted keys gives the body again.

A JSON parser that changes the text of a number breaks the check. In Node.js, `verifyIpnSignature` from the [TypeScript SDK](/docs/sdk/) keeps the text of each number. The Go example keeps it with `UseNumber`. The PHP, Python and Ruby examples keep integers and decimals from 0.0001 up. They can change a smaller number. For such amounts, use the timestamp signature.

The examples read the default header `x-turmite-sig`. If you changed the header name in **Settings**, read that name.

### Node.js

```js
import { verifyIpnSignature } from '@turmite/sdk';

// In an Express route with express.raw({ type: 'application/json' }):
const ok = await verifyIpnSignature(req.body, req.headers['x-turmite-sig'], process.env.TM_IPN_SECRET);
if (!ok) return res.sendStatus(401);
const payment = JSON.parse(req.body.toString('utf8'));
```

### PHP

```php
<?php
function tm_sort_keys($value) {
    if (is_array($value)) {
        if (!array_is_list($value)) {
            ksort($value, SORT_STRING);
        }
        foreach ($value as $key => $item) {
            $value[$key] = tm_sort_keys($item);
        }
    }
    return $value;
}

$secret = getenv('TM_IPN_SECRET');
$raw = file_get_contents('php://input');
$sig = strtolower($_SERVER['HTTP_X_TURMITE_SIG'] ?? '');
$data = json_decode($raw, true);

$canonical = json_encode(tm_sort_keys($data), JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE);
$want = hash_hmac('sha512', $canonical, $secret);
if (!is_array($data) || !hash_equals($want, $sig)) {
    http_response_code(401);
    exit;
}
// $data is the payment object. Update the order here.
http_response_code(200);
```

`array_is_list` needs PHP 8.1 or later.

### Python

```python
import hashlib
import hmac
import json


def verify_compat(raw: bytes, signature: str, ipn_secret: bytes) -> bool:
    canonical = json.dumps(
        json.loads(raw), sort_keys=True, separators=(",", ":"), ensure_ascii=False
    )
    want = hmac.new(ipn_secret, canonical.encode(), hashlib.sha512).hexdigest()
    return hmac.compare_digest(want, (signature or "").lower())


# Flask: verify_compat(request.get_data(), request.headers.get("x-turmite-sig"), SECRET)
```

### Go

```go
package webhook

import (
	"bytes"
	"crypto/hmac"
	"crypto/sha512"
	"encoding/hex"
	"encoding/json"
	"strings"
)

// Go encodes map keys in sorted order. UseNumber keeps each number as it was sent.
func VerifyCompat(body []byte, signature string, ipnSecret []byte) bool {
	dec := json.NewDecoder(bytes.NewReader(body))
	dec.UseNumber()
	var v any
	if err := dec.Decode(&v); err != nil {
		return false
	}
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		return false
	}
	canonical := bytes.TrimSuffix(buf.Bytes(), []byte("\n"))
	mac := hmac.New(sha512.New, ipnSecret)
	mac.Write(canonical)
	want := hex.EncodeToString(mac.Sum(nil))
	return hmac.Equal([]byte(want), []byte(strings.ToLower(signature)))
}

// In a handler: VerifyCompat(body, r.Header.Get("X-Turmite-Sig"), secret)
```

### Ruby

```ruby
require "json"
require "openssl"

def sort_keys(value)
  case value
  when Hash then value.keys.sort.to_h { |k| [k, sort_keys(value[k])] }
  when Array then value.map { |v| sort_keys(v) }
  else value
  end
end

def verify_compat(raw, signature, ipn_secret)
  canonical = JSON.generate(sort_keys(JSON.parse(raw)))
  want = OpenSSL::HMAC.hexdigest("SHA512", ipn_secret, canonical)
  OpenSSL.secure_compare(want, signature.to_s.downcase)
end

# Sinatra: verify_compat(request.body.read, request.env["HTTP_X_TURMITE_SIG"], ENV.fetch("TM_IPN_SECRET"))
```

## Delivery rules

- Turmite sends `POST` with a JSON body.
- A `2xx` response marks the attempt as delivered. Any other response or a timeout is a failure.
- Turmite does not follow redirects. A `3xx` response is a failure.
- On mainnet, the URL must be an `https` URL. Turmite refuses private and reserved IP addresses when it connects, so a DNS record that points to a private address does not work.
- On testnet, `http` URLs and private addresses such as `localhost` are allowed for testing.
- Each attempt has a timeout of 30 seconds, with 10 seconds to connect.

Respond quickly. Do slow work, such as emails, after you send the response.

## Retries

| Kind | Attempts |
|---|---|
| Progress events | 3: now, then after 10 s and 60 s |
| Final events | 11: now, then after 10 s, 1 m, 5 m, 30 m, 1 h, 2 h, 4 h, 8 h, 24 h and 48 h |

After the last attempt, the delivery status becomes `dead`. A later status change sends a new event, so Turmite retries progress events fewer times.

| Final events | Progress events |
|---|---|
| `payment.finished`, `payment.refunded`, `payment.expired`, `payment.failed` | `payment.waiting`, `payment.confirming`, `payment.confirmed`, `payment.partially_paid` |
| `payout.sent`, `payout.failed`, `payout.rejected`, `payout.cancelled` | The other `payout.*` events, `payout.needs-review` included |
| | `test` |

`payment.partially_paid` is a progress event: the customer can still send the rest. `payment.failed` is in the list for compatibility. Turmite does not set the `failed` status on payments.

To send a delivery again, open **Webhooks**, find it under **Deliveries** and click **Retry**. Retry works for every delivery that is not in progress, also a delivered one. The list shows the last 50 deliveries.

## Handle events safely

- Make your handler idempotent. The same delivery can arrive more than one time. Store `x-turmite-delivery` and ignore repeats. Each target gets its own delivery ID for the same event.
- Do not depend on the order of deliveries. Use `payment_status` and `updated_at` from the signed body. Ignore a status that is older than the one you stored.
- Take the event from the signed body. `x-turmite-event` and `x-turmite-delivery` are not signed.
- Check that `order_id` and `price_amount` match your order.
- Ship the order on `finished` only.
- For a large order, read the payment again with `GET /v1/payment/{payment_id}` before you ship.

## Test webhooks

1. Open **Webhooks** in the dashboard.
2. Find the endpoint under **Endpoints**.
3. Click **Send test**. Turmite sends a `test` event, signed with the endpoint secret.
4. Look under **Deliveries** for the result and the number of tries.

**Send test** works for project endpoints only.

On a testnet install, pay with testnet coins to see real `payment.*` events. On your own computer, run Turmite with `turmite serve --demo` and point the endpoint at your local receiver. See [Quick start](/docs/quick-start/).

A signature mismatch has two usual causes:

- The framework parsed the body before your check. Read the raw bytes, for example with `express.raw()`.
- The secret is wrong or has extra spaces.
