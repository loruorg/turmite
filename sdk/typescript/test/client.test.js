import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { Turmite, TurmiteError } from '../dist/index.js';

function mock(responses = []) {
  const calls = [];
  const fetch = async (url, init) => {
    calls.push({ url, init, body: init.body === undefined ? undefined : JSON.parse(init.body) });
    const next = responses.shift() ?? { status: 200, body: {} };
    if (next instanceof Error) throw next;
    const text = typeof next.body === 'string' ? next.body : JSON.stringify(next.body);
    return new Response(text, { status: next.status ?? 200, statusText: next.statusText ?? '' });
  };
  return { fetch, calls };
}

const client = (m, extra = {}) =>
  new Turmite({ baseUrl: 'https://shop.example/pay/', apiKey: 'tm_test', fetch: m.fetch, ...extra });

describe('Turmite client', () => {
  it('joins the base URL path and sends the API key', async () => {
    const m = mock([{ body: { message: 'OK' } }]);
    assert.deepEqual(await client(m).status(), { message: 'OK' });
    assert.equal(m.calls[0].url, 'https://shop.example/pay/v1/status');
    assert.equal(m.calls[0].init.method, 'GET');
    assert.equal(m.calls[0].init.headers['x-api-key'], 'tm_test');
    assert.equal(m.calls[0].init.headers.authorization, undefined);
  });

  it('maps every method to its path', async () => {
    const m = mock();
    const t = client(m);
    await t.currencies();
    await t.fullCurrencies();
    await t.merchantCoins();
    await t.minAmount({ currency_from: 'btc' });
    await t.estimate({ amount: 10.5, currency_from: 'usd', currency_to: 'usdttrc20' });
    await t.getPayment(42);
    await t.listPayments({ limit: 10, page: 2, sortBy: 'created_at', orderBy: 'asc', dateFrom: undefined, invoiceId: 9 });
    await t.listPayments();
    await t.balance();
    await t.getPayout('77');
    await t.validateAddress({ address: 'T9yD', currency: 'usdttrc20' });
    await t.payoutFee({ currency: 'trx', amount: '25' });
    assert.deepEqual(
      m.calls.map((c) => `${c.init.method} ${c.url.replace('https://shop.example/pay', '')}`),
      [
        'GET /v1/currencies',
        'GET /v1/full-currencies',
        'GET /v1/merchant/coins',
        'GET /v1/min-amount?currency_from=btc',
        'GET /v1/estimate?amount=10.5&currency_from=usd&currency_to=usdttrc20',
        'GET /v1/payment/42',
        'GET /v1/payment?limit=10&page=2&sortBy=created_at&orderBy=asc&invoiceId=9',
        'GET /v1/payment',
        'GET /v1/balance',
        'GET /v1/payout/77',
        'POST /v1/payout/validate-address',
        'GET /v1/payout/fee?currency=trx&amount=25',
      ],
    );
    assert.deepEqual(m.calls[10].body, { address: 'T9yD', currency: 'usdttrc20' });
  });

  it('creates payments, invoices and invoice payments (201)', async () => {
    const m = mock([
      { status: 201, body: { payment_id: '1', purchase_id: '6084815412', payment_status: 'waiting' } },
      { status: 201, body: { id: '9', invoice_url: 'https://shop.example/pay/i/abc' } },
      { status: 201, body: { payment_id: '2', invoice_id: 9 } },
    ]);
    const t = client(m);
    const payment = await t.createPayment({ price_amount: 10, price_currency: 'usd', pay_currency: 'btc', order_id: 'A1' });
    assert.equal(payment.payment_id, '1');
    const invoice = await t.createInvoice({ price_amount: '19.99', price_currency: 'eur', order_id: 'A2' });
    assert.equal(invoice.invoice_url, 'https://shop.example/pay/i/abc');
    await t.createInvoicePayment({ iid: 9, pay_currency: 'usdcbase' });
    assert.equal(m.calls[0].init.headers['content-type'], 'application/json');
    assert.deepEqual(m.calls[0].body, { price_amount: 10, price_currency: 'usd', pay_currency: 'btc', order_id: 'A1' });
    assert.equal(m.calls[1].url, 'https://shop.example/pay/v1/invoice');
    assert.equal(m.calls[2].url, 'https://shop.example/pay/v1/invoice-payment');
    assert.deepEqual(m.calls[2].body, { iid: 9, pay_currency: 'usdcbase' });
  });

  it('keeps the auth token for payout calls', async () => {
    const m = mock([{ body: { token: 'jwt1' } }, { status: 201, body: { id: '5', withdrawals: [] } }, { body: { id: '5', withdrawals: [] } }, { body: { payouts: [] } }, { body: { id: '5', withdrawals: [] } }]);
    const t = client(m);
    assert.deepEqual(await t.auth('ops@shop.example', 'pw'), { token: 'jwt1' });
    assert.deepEqual(m.calls[0].body, { email: 'ops@shop.example', password: 'pw' });
    await t.createPayout({ withdrawals: [{ address: 'T9yD', currency: 'usdttrc20', amount: '12.5' }] });
    await t.verifyPayout('5', '123456');
    await t.listPayouts({ status: 'sent' });
    await t.createPayout({ withdrawals: [] }, { token: 'other' });
    assert.equal(m.calls[1].url, 'https://shop.example/pay/v1/payout');
    assert.equal(m.calls[1].init.headers.authorization, 'Bearer jwt1');
    assert.equal(m.calls[1].init.headers['x-api-key'], 'tm_test');
    assert.equal(m.calls[2].url, 'https://shop.example/pay/v1/payout/5/verify');
    assert.deepEqual(m.calls[2].body, { verification_code: '123456' });
    assert.equal(m.calls[3].url, 'https://shop.example/pay/v1/payout?status=sent');
    assert.equal(m.calls[3].init.headers.authorization, 'Bearer jwt1');
    assert.equal(m.calls[4].init.headers.authorization, 'Bearer other');
  });

  it('needs a token for payout calls', async () => {
    const m = mock();
    await assert.rejects(client(m).createPayout({ withdrawals: [] }), (err) => {
      assert.ok(err instanceof TurmiteError);
      assert.equal(err.code, 'TOKEN_REQUIRED');
      return true;
    });
    assert.equal(m.calls.length, 0);
  });

  it('throws TurmiteError with the API error fields', async () => {
    const m = mock([
      { status: 400, body: { status: false, statusCode: 400, code: 'INVALID_REQUEST_PARAMS', message: 'Currency `xmr` is not enabled for this project.' } },
      { status: 401, body: { status: false, message: 'Invalid API key.' } },
      { status: 502, statusText: 'Bad Gateway', body: '<html>bad gateway</html>' },
      { status: 503, statusText: 'Service Unavailable', body: '' },
    ]);
    const t = client(m);
    const expect = async (status, code, message) => {
      await assert.rejects(t.status(), (err) => {
        assert.ok(err instanceof TurmiteError);
        assert.equal(err.name, 'TurmiteError');
        assert.equal(err.status, status);
        assert.equal(err.code, code);
        assert.equal(err.message, message);
        return true;
      });
    };
    await expect(400, 'INVALID_REQUEST_PARAMS', 'Currency `xmr` is not enabled for this project.');
    await expect(401, 'HTTP_401', 'Invalid API key.');
    await expect(502, 'HTTP_502', '<html>bad gateway</html>');
    await expect(503, 'HTTP_503', '503 Service Unavailable');
  });

  it('wraps network errors and timeouts', async () => {
    const m = mock([new TypeError('fetch failed')]);
    await assert.rejects(client(m).status(), (err) => err instanceof TurmiteError && err.code === 'NETWORK_ERROR' && err.status === 0);

    const slow = (_url, init) =>
      new Promise((_resolve, reject) => init.signal.addEventListener('abort', () => reject(init.signal.reason)));
    const t = new Turmite({ baseUrl: 'https://shop.example', fetch: slow, timeoutMs: 20 });
    await assert.rejects(t.status(), (err) => err instanceof TurmiteError && err.code === 'TIMEOUT');
  });

  it('uses the global fetch when none is given', async () => {
    const original = globalThis.fetch;
    const m = mock([{ body: { currencies: ['btc'] } }]);
    globalThis.fetch = m.fetch;
    try {
      const t = new Turmite({ baseUrl: 'https://shop.example' });
      assert.deepEqual(await t.currencies(), { currencies: ['btc'] });
      assert.equal(m.calls[0].init.headers['x-api-key'], undefined);
    } finally {
      globalThis.fetch = original;
    }
  });

  it('requires a base URL', () => {
    assert.throws(() => new Turmite({}), TypeError);
  });
});
