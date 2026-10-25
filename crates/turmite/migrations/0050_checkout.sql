-- Checkout: branding, return URLs, checkout domains and the coin exchange.

ALTER TABLE projects
  ADD COLUMN logo_png          BYTEA,
  ADD COLUMN logo_hash         TEXT,
  ADD COLUMN logo_url          TEXT,
  ADD COLUMN accent_color      TEXT CHECK (accent_color ~ '^#[0-9a-f]{6}$'),
  ADD COLUMN checkout_theme    TEXT NOT NULL DEFAULT 'auto' CHECK (checkout_theme IN ('light', 'dark', 'auto')),
  ADD COLUMN checkout_message  TEXT CHECK (LENGTH(checkout_message) <= 280),
  ADD COLUMN support_email     TEXT,
  ADD COLUMN support_url       TEXT,
  ADD COLUMN return_url        TEXT,
  ADD COLUMN return_hosts      TEXT[] NOT NULL DEFAULT '{}',
  ADD COLUMN checkout_domain   TEXT CHECK (checkout_domain = LOWER(checkout_domain)),
  ADD COLUMN exchange_enabled  BOOLEAN NOT NULL DEFAULT false,
  ADD COLUMN trocador_key      BYTEA,
  ADD COLUMN exchange_markup   NUMERIC(4,2) NOT NULL DEFAULT 0 CHECK (exchange_markup IN (0, 1, 1.65, 3)),
  ADD COLUMN exchange_min_kyc  TEXT NOT NULL DEFAULT 'B' CHECK (exchange_min_kyc IN ('A', 'B', 'C', 'D')),
  ADD COLUMN exchange_coins    TEXT[] NOT NULL DEFAULT '{}';
CREATE UNIQUE INDEX projects_checkout_domain_key ON projects (checkout_domain) WHERE checkout_domain IS NOT NULL;

-- The checkout animates from the time of the last status change.
ALTER TABLE payments
  ADD COLUMN status_changed_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  ADD COLUMN return_url         TEXT;

CREATE FUNCTION payments_status_changed() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  NEW.status_changed_at := now();
  RETURN NEW;
END $$;
CREATE TRIGGER payments_status_changed BEFORE UPDATE OF status ON payments
  FOR EACH ROW WHEN (OLD.status IS DISTINCT FROM NEW.status) EXECUTE FUNCTION payments_status_changed();

-- A payment keeps the project's return URL of the time it was created.
CREATE FUNCTION payments_return_url() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.return_url IS NULL THEN
    SELECT return_url INTO NEW.return_url FROM projects WHERE project_id = NEW.project_id;
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER payments_return_url BEFORE INSERT ON payments
  FOR EACH ROW EXECUTE FUNCTION payments_return_url();

-- Coins the exchange lists. Shared by all projects, refreshed once a day.
CREATE TABLE exchange_coins (
  ticker      TEXT NOT NULL,
  network     TEXT NOT NULL,
  name        TEXT NOT NULL,
  memo        BOOLEAN NOT NULL DEFAULT false,
  minimum     NUMERIC(38,18),
  maximum     NUMERIC(38,18),
  updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (ticker, network)
);

-- Swaps a customer started to pay a payment in another coin. The status is shown
-- on the checkout only; the payment is credited by the chain watcher.
CREATE TABLE exchange_trades (
  exchange_trade_id      BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  payment_id             BIGINT NOT NULL REFERENCES payments(payment_id) ON DELETE CASCADE,
  project_id             BIGINT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  token                  TEXT NOT NULL UNIQUE,
  hook_secret            TEXT NOT NULL UNIQUE,
  webhook_key            TEXT NOT NULL,
  trade_id               TEXT NOT NULL,
  provider               TEXT NOT NULL,
  kyc_rating             TEXT,
  coin_from              TEXT NOT NULL,
  ticker_from            TEXT NOT NULL,
  network_from           TEXT NOT NULL,
  ticker_to              TEXT NOT NULL,
  network_to             TEXT NOT NULL,
  amount_from            NUMERIC(38,18) NOT NULL,
  amount_to              NUMERIC(38,18) NOT NULL,
  address_provider       TEXT NOT NULL,
  address_provider_memo  TEXT,
  refund_address         TEXT,
  refund_memo            TEXT,
  status                 TEXT NOT NULL,
  hashout                TEXT,
  expires_at             TIMESTAMPTZ,
  status_changed_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
  polled_at              TIMESTAMPTZ,
  created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at             TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX exchange_trades_payment_idx ON exchange_trades (payment_id, created_at DESC);
CREATE INDEX exchange_trades_active_idx ON exchange_trades (polled_at NULLS FIRST)
  WHERE status IN ('new', 'waiting', 'confirming', 'sending');
