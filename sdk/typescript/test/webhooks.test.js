import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createHmac } from 'node:crypto';
import { describe, it } from 'node:test';

import {
  WebhookVerificationError,
  canonicalJson,
  isPaid,
  verifyIpnSignature,
  verifyWebhook,
} from '../dist/index.js';

// Computed with the server's signing code. The server now writes numbers as JavaScript does
// (`js_numbers`); some cases keep number text of older servers (`1e-6`, `100.0`).
const cases = JSON.parse(readFileSync(new URL('./fixtures/signatures.json', import.meta.url), 'utf8'));

const tamper = (body) => body.replace(/(\d)(?=[,}\]])/, (d) => String((Number(d) + 1) % 10));

describe('verifyIpnSignature', () => {
  for (const c of cases) {
    it(`${c.name}: accepts the raw body in every form`, async () => {
      assert.equal(await verifyIpnSignature(c.body, c.sig, c.secret), true);
      assert.equal(await verifyIpnSignature(c.body_reversed, c.sig, c.secret), true);
      assert.equal(await verifyIpnSignature(c.body_pretty, c.sig, c.secret), true);
      assert.equal(await verifyIpnSignature(new TextEncoder().encode(c.body), c.sig, c.secret), true);
      assert.equal(await verifyIpnSignature(Buffer.from(c.body_reversed), c.sig, c.secret), true);
      assert.equal(await verifyIpnSignature(new TextEncoder().encode(c.body).buffer, c.sig, c.secret), true);
      assert.equal(await verifyIpnSignature(c.body, c.sig.toUpperCase(), c.secret), true);
    });

    it(`${c.name}: canonical form equals the server's sorted body`, () => {
      assert.equal(canonicalJson(c.body_reversed), c.body);
      assert.equal(canonicalJson(c.body_pretty), c.body);
    });

    it(`${c.name}: rejects a wrong secret, a changed body or a bad signature`, async () => {
      assert.equal(await verifyIpnSignature(c.body, c.sig, `${c.secret}x`), false);
      assert.equal(await verifyIpnSignature(tamper(c.body), c.sig, c.secret), false);
      assert.equal(await verifyIpnSignature(c.body, c.sig.slice(0, -2), c.secret), false);
      assert.equal(await verifyIpnSignature(c.body, `${c.sig.slice(0, -1)}g`, c.secret), false);
      assert.equal(await verifyIpnSignature(c.body, '', c.secret), false);
      assert.equal(await verifyIpnSignature(c.body, undefined, c.secret), false);
      assert.equal(await verifyIpnSignature('{"a":', c.sig, c.secret), false);
    });

    it(`${c.name}: a parsed object passes only when its numbers print the same`, async () => {
      assert.equal(await verifyIpnSignature(JSON.parse(c.body), c.sig, c.secret), c.object_input_ok);
    });
  }

  it('sorts keys by UTF-8 bytes, not UTF-16 units', () => {
    assert.equal(canonicalJson('{"\u{1F600}":1,"｡":2,"b":3,"B":4,"":5}'), '{"":5,"B":4,"b":3,"｡":2,"\u{1F600}":1}');
  });

  it('keeps number text and escapes strings like the server', () => {
    assert.equal(canonicalJson('{ "b" : 1.0 , "a" : [ 1e+16 , -0.0 , 5e-324 ] }'), '{"a":[1e+16,-0.0,5e-324],"b":1.0}');
    assert.equal(canonicalJson('{"s":"\\u00e9\\/\\u0001\\u001F\\u2028"}'), '{"s":"\u00e9/\\u0001\\u001f\u2028"}');
  });

  it('rejects invalid JSON', () => {
    for (const bad of ['', '{', '{"a":1,}', '[1 2]', '{"a":01}', '{"a":1.}', '"\u0001"', '{"a":tru}', '{} {}']) {
      assert.throws(() => canonicalJson(bad), SyntaxError, bad);
    }
  });

  it('rejects an empty secret', async () => {
    await assert.rejects(verifyIpnSignature('{}', 'ab'.repeat(64), ''), TypeError);
  });

  it('matches a signature made with node:crypto', async () => {
    const body = '{"z":1,"a":{"y":[2,{"d":0.5,"c":null}]}}';
    const sig = createHmac('sha512', 'k').update('{"a":{"y":[2,{"c":null,"d":0.5}]},"z":1}').digest('hex');
    assert.equal(await verifyIpnSignature(body, sig, 'k'), true);
  });
});

describe('verifyWebhook', () => {
  const c = cases.find((x) => x.name === 'endpoint_envelope');
  const now = c.timestamp * 1000;
  const headers = { 'X-Turmite-Timestamp': String(c.timestamp), 'x-turmite-signature': c.signature_v1 };

  it('returns the parsed body', async () => {
    const event = await verifyWebhook(c.body, headers, c.secret, { now });
    assert.equal(event.event, 'payment.finished');
    assert.equal(event.data.payment_status, 'finished');
  });

  it('accepts Headers, bytes and several v1 values', async () => {
    const h = new Headers({
      'x-turmite-timestamp': String(c.timestamp),
      'x-turmite-signature': `v1=${'0'.repeat(64)}, ${c.signature_v1}`,
    });
    const event = await verifyWebhook(new TextEncoder().encode(c.body), h, c.secret, { now: now + 299_000 });
    assert.equal(event.delivery_id, 'dlv01JBZ8Q2N4hT6sWq3mXk9a');
  });

  it('works with every fixture', async () => {
    for (const x of cases) {
      const h = { 'x-turmite-timestamp': String(x.timestamp), 'x-turmite-signature': x.signature_v1 };
      await verifyWebhook(x.body, h, x.secret, { now: x.timestamp * 1000 });
    }
  });

  const fails = async (code, body, h, opts = { now }) => {
    await assert.rejects(verifyWebhook(body, h, c.secret, opts), (err) => {
      assert.ok(err instanceof WebhookVerificationError);
      assert.equal(err.code, code);
      return true;
    });
  };

  it('rejects old, future and missing timestamps', async () => {
    await fails('timestamp_out_of_range', c.body, headers, { now: now + 301_000 });
    await fails('timestamp_out_of_range', c.body, headers, { now: now - 301_000 });
    await fails('timestamp_out_of_range', c.body, headers, { now: now + 61_000, toleranceSeconds: 60 });
    await fails('missing_header', c.body, { 'x-turmite-signature': c.signature_v1 });
    await fails('invalid_timestamp', c.body, { ...headers, 'X-Turmite-Timestamp': '17e8' });
  });

  it('rejects a changed body, a wrong secret or a bad signature', async () => {
    await fails('invalid_signature', tamper(c.body), headers);
    await fails('invalid_signature', c.body_reversed, headers);
    await fails('invalid_signature', c.body, { ...headers, 'x-turmite-signature': c.signature_v1.slice(3) });
    await fails('invalid_signature', c.body, { ...headers, 'x-turmite-signature': 'v1=zz' });
    await assert.rejects(verifyWebhook(c.body, headers, 'other', { now }), WebhookVerificationError);
  });

  it('matches a signature made with node:crypto', async () => {
    const body = '{"event":"test","delivery_id":"d1","created_at":"2026-10-23T00:00:00Z","data":{}}';
    const sig = createHmac('sha256', 's').update(`1700000000.${body}`).digest('hex');
    const h = { 'x-turmite-timestamp': '1700000000', 'x-turmite-signature': `v1=${sig}` };
    const event = await verifyWebhook(body, h, 's', { now: 1_700_000_000_000 });
    assert.equal(event.event, 'test');
  });
});

describe('isPaid', () => {
  it('is true only for finished', () => {
    assert.equal(isPaid('finished'), true);
    for (const s of ['waiting', 'confirming', 'confirmed', 'sending', 'partially_paid', 'failed', 'refunded', 'expired']) {
      assert.equal(isPaid(s), false, s);
    }
  });
});
