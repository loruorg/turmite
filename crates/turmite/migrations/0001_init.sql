-- Core schema: install settings, users, projects, catalog, payments, webhooks.

CREATE TYPE network_kind AS ENUM ('utxo', 'evm', 'account', 'account_memo');

-- Install-wide key/value settings (deposit seed, update check state, ...).
CREATE TABLE settings (
  key         TEXT PRIMARY KEY,
  value       JSONB NOT NULL,
  updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---- Users and sessions ----------------------------------------------------

CREATE TABLE users (
  user_id         BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  email           TEXT NOT NULL,
  password_hash   TEXT NOT NULL,
  role            TEXT NOT NULL CHECK (role IN ('root', 'admin', 'member')),
  totp_secret     BYTEA,
  totp_enabled    BOOLEAN NOT NULL DEFAULT false,
  totp_last_step  BIGINT NOT NULL DEFAULT 0,
  disabled_at     TIMESTAMPTZ,
  last_login_at   TIMESTAMPTZ,
  created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX users_email_key ON users (LOWER(email));
CREATE UNIQUE INDEX users_one_root ON users ((role)) WHERE role = 'root';

CREATE TABLE sessions (
  session_hash  TEXT PRIMARY KEY,
  user_id       BIGINT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
  ip            TEXT,
  user_agent    TEXT,
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_seen_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at    TIMESTAMPTZ NOT NULL
);
CREATE INDEX sessions_user_idx ON sessions (user_id);

-- ---- Projects --------------------------------------------------------------

CREATE TABLE projects (
  project_id              BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  name                    TEXT NOT NULL CHECK (LENGTH(name) BETWEEN 1 AND 80),
  ipn_secret              BYTEA NOT NULL,
  signature_header        TEXT NOT NULL DEFAULT 'x-turmite-sig',
  payment_expiry_minutes  INTEGER NOT NULL DEFAULT 60 CHECK (payment_expiry_minutes BETWEEN 5 AND 10080),
  invoice_expiry_minutes  INTEGER NOT NULL DEFAULT 1440 CHECK (invoice_expiry_minutes BETWEEN 5 AND 43200),
  created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE project_members (
  project_id  BIGINT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  user_id     BIGINT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
  role        TEXT NOT NULL CHECK (role IN ('project_admin', 'developer', 'read_only')),
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (project_id, user_id)
);

CREATE TABLE api_keys (
  api_key_id    BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  project_id    BIGINT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  label         TEXT NOT NULL,
  token_hash    TEXT NOT NULL,
  token_last4   TEXT NOT NULL,
  created_by    BIGINT REFERENCES users(user_id) ON DELETE SET NULL,
  last_used_at  TIMESTAMPTZ,
  revoked_at    TIMESTAMPTZ,
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX api_keys_token_hash_key ON api_keys (token_hash);
CREATE INDEX api_keys_project_idx ON api_keys (project_id);

CREATE TABLE audit_log (
  audit_id    BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  user_id     BIGINT REFERENCES users(user_id) ON DELETE SET NULL,
  project_id  BIGINT REFERENCES projects(project_id) ON DELETE SET NULL,
  actor       TEXT NOT NULL CHECK (actor IN ('user', 'api_key', 'system')),
  action      TEXT NOT NULL,
  entity      TEXT NOT NULL,
  entity_id   TEXT,
  data        JSONB NOT NULL DEFAULT '{}',
  ip          TEXT,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX audit_log_created_idx ON audit_log (created_at DESC);
CREATE INDEX audit_log_project_idx ON audit_log (project_id, created_at DESC);

-- ---- Catalog: assets, networks, nodes ---------------------------------------

CREATE TABLE asset (
  asset_id      BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  symbol        TEXT NOT NULL,
  name          TEXT NOT NULL,
  coingecko_id  TEXT,
  is_stable     BOOLEAN NOT NULL DEFAULT false
);
CREATE UNIQUE INDEX asset_symbol_key ON asset (LOWER(symbol));

CREATE TABLE network (
  network_id              BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  code                    TEXT NOT NULL UNIQUE,
  chain                   TEXT NOT NULL,
  family                  TEXT NOT NULL CHECK (family IN ('bitcoin', 'monero', 'evm', 'solana', 'tron', 'ton')),
  name                    TEXT NOT NULL,
  chain_id                BIGINT,
  kind                    network_kind NOT NULL,
  native_asset_id         BIGINT NOT NULL REFERENCES asset(asset_id),
  confirmations_required  INTEGER NOT NULL CHECK (confirmations_required >= 0),
  explorer_tx_url         TEXT,
  explorer_address_url    TEXT,
  is_testnet              BOOLEAN NOT NULL,
  is_enabled              BOOLEAN NOT NULL DEFAULT true,
  last_block              BIGINT,
  last_block_at           TIMESTAMPTZ,
  created_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX network_chain_key ON network (chain);

CREATE TABLE asset_network (
  asset_network_id    BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  asset_id            BIGINT NOT NULL REFERENCES asset(asset_id),
  network_id          BIGINT NOT NULL REFERENCES network(network_id),
  ticker              TEXT NOT NULL,
  decimals            INTEGER NOT NULL CHECK (decimals BETWEEN 0 AND 30),
  contract_address    TEXT,
  min_payment_amount  NUMERIC(38,18),
  is_enabled          BOOLEAN NOT NULL DEFAULT true,
  created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX asset_network_ticker_key ON asset_network (ticker);
CREATE UNIQUE INDEX asset_network_pair_key ON asset_network (network_id, asset_id);
CREATE UNIQUE INDEX asset_network_one_native ON asset_network (network_id) WHERE contract_address IS NULL;

-- RPC nodes per network. The URL can hold an API key, so it is sealed.
CREATE TABLE nodes (
  node_id         BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  network_id      BIGINT NOT NULL REFERENCES network(network_id) ON DELETE CASCADE,
  label           TEXT NOT NULL,
  url_sealed      BYTEA NOT NULL,
  url_display     TEXT NOT NULL,
  priority        INTEGER NOT NULL DEFAULT 0,
  is_enabled      BOOLEAN NOT NULL DEFAULT true,
  last_ok_at      TIMESTAMPTZ,
  last_error      TEXT,
  last_error_at   TIMESTAMPTZ,
  last_height     BIGINT,
  created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX nodes_network_idx ON nodes (network_id, priority);

-- ---- Settlement wallets and deposit addresses --------------------------------

-- Where a project's funds go on one network. BTC uses `xpub` (watch-only),
-- XMR uses `address` + sealed `view_key`, EVM uses `address` as the cold wallet
-- fixed in `factory_address`, other chains sweep to `address`.
CREATE TABLE project_wallets (
  project_wallet_id  BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  project_id         BIGINT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  network_id         BIGINT NOT NULL REFERENCES network(network_id),
  address            TEXT,
  address_extra_id   TEXT,
  xpub               TEXT,
  view_key           BYTEA,
  restore_height     BIGINT,
  factory_address    TEXT,
  next_index         BIGINT NOT NULL DEFAULT 0,
  is_enabled         BOOLEAN NOT NULL DEFAULT true,
  created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (project_id, network_id)
);

-- Deposit addresses. Rows with no payment are the unused pool.
CREATE TABLE deposit_addresses (
  deposit_address_id  BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  network_id          BIGINT NOT NULL REFERENCES network(network_id),
  project_id          BIGINT REFERENCES projects(project_id) ON DELETE RESTRICT,
  derivation_index    BIGINT NOT NULL CHECK (derivation_index >= 0),
  address             TEXT NOT NULL,
  payin_extra_id      TEXT,
  payment_id          BIGINT,
  reserved_at         TIMESTAMPTZ,
  is_swept            BOOLEAN NOT NULL DEFAULT false,
  created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX deposit_addresses_address_key
  ON deposit_addresses (network_id, address, COALESCE(payin_extra_id, ''));
CREATE UNIQUE INDEX deposit_addresses_payment_key ON deposit_addresses (payment_id) WHERE payment_id IS NOT NULL;
CREATE INDEX deposit_addresses_pool_idx ON deposit_addresses (network_id, project_id) WHERE payment_id IS NULL;

-- Install-wide derivation counter per network (seed-derived chains).
CREATE TABLE network_address_counter (
  network_id  BIGINT PRIMARY KEY REFERENCES network(network_id),
  next_index  BIGINT NOT NULL DEFAULT 1
);

-- ---- Invoices and payments ---------------------------------------------------

CREATE TABLE invoices (
  invoice_id         BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  project_id         BIGINT NOT NULL REFERENCES projects(project_id) ON DELETE RESTRICT,
  token              TEXT NOT NULL UNIQUE,
  order_id           TEXT,
  order_description  TEXT,
  price_amount       NUMERIC(38,18) NOT NULL CHECK (price_amount > 0),
  price_currency     TEXT NOT NULL,
  pay_currency       TEXT,
  ipn_callback_url   TEXT,
  success_url        TEXT,
  cancel_url         TEXT,
  expires_at         TIMESTAMPTZ NOT NULL,
  created_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX invoices_project_idx ON invoices (project_id, created_at DESC);

CREATE TABLE payments (
  payment_id          BIGINT GENERATED ALWAYS AS IDENTITY (START WITH 4100000001) PRIMARY KEY,
  project_id          BIGINT NOT NULL REFERENCES projects(project_id) ON DELETE RESTRICT,
  invoice_id          BIGINT REFERENCES invoices(invoice_id) ON DELETE RESTRICT,
  token               TEXT NOT NULL UNIQUE,
  asset_network_id    BIGINT NOT NULL REFERENCES asset_network(asset_network_id),
  network_id          BIGINT NOT NULL REFERENCES network(network_id),
  deposit_address_id  BIGINT REFERENCES deposit_addresses(deposit_address_id),
  idempotency_key     TEXT,
  purchase_id         TEXT NOT NULL,
  order_id            TEXT,
  order_description   TEXT,
  price_amount        NUMERIC(38,18) NOT NULL CHECK (price_amount > 0),
  price_currency      TEXT NOT NULL,
  pay_address         TEXT NOT NULL,
  payin_extra_id      TEXT,
  pay_amount          NUMERIC(38,18) NOT NULL CHECK (pay_amount > 0),
  actually_paid       NUMERIC(38,18) NOT NULL DEFAULT 0 CHECK (actually_paid >= 0),
  outcome_amount      NUMERIC(38,18),
  network_fee_amount  NUMERIC(38,18) NOT NULL DEFAULT 0 CHECK (network_fee_amount >= 0),
  swept_gross         NUMERIC(38,18) NOT NULL DEFAULT 0 CHECK (swept_gross >= 0),
  status              TEXT NOT NULL DEFAULT 'waiting'
                        CHECK (status IN ('waiting', 'confirming', 'confirmed', 'sending',
                                          'finished', 'partially_paid', 'failed', 'expired', 'refunded')),
  settlement          TEXT NOT NULL DEFAULT 'none'
                        CHECK (settlement IN ('none', 'underpaid', 'overpaid', 'paid_late',
                                              'marked_paid', 'refunded')),
  confirmations       INTEGER NOT NULL DEFAULT 0 CHECK (confirmations >= 0),
  ipn_callback_url    TEXT,
  success_url         TEXT,
  cancel_url          TEXT,
  rate                NUMERIC(38,18),
  expires_at          TIMESTAMPTZ NOT NULL,
  paid_at             TIMESTAMPTZ,
  created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX payments_idempotency_key ON payments (project_id, idempotency_key) WHERE idempotency_key IS NOT NULL;
CREATE INDEX payments_project_idx ON payments (project_id, created_at DESC);
CREATE INDEX payments_invoice_idx ON payments (invoice_id);
CREATE INDEX payments_active_idx ON payments (status) WHERE status IN ('waiting', 'confirming', 'partially_paid', 'confirmed', 'sending');
CREATE INDEX payments_address_idx ON payments (network_id, pay_address);

ALTER TABLE deposit_addresses
  ADD CONSTRAINT deposit_addresses_payment_fk FOREIGN KEY (payment_id) REFERENCES payments(payment_id) ON DELETE SET NULL;

-- Incoming on-chain transfers to deposit addresses.
CREATE TABLE payment_transactions (
  payment_tx_id   BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  payment_id      BIGINT REFERENCES payments(payment_id) ON DELETE SET NULL,
  network_id      BIGINT NOT NULL REFERENCES network(network_id),
  chain_tx_key    TEXT NOT NULL,
  txid            TEXT NOT NULL,
  vout            INTEGER,
  amount          NUMERIC(38,18) NOT NULL CHECK (amount >= 0),
  confirmations   INTEGER NOT NULL DEFAULT 0 CHECK (confirmations >= 0),
  block_height    BIGINT,
  block_hash      TEXT,
  seen_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
  confirmed_at    TIMESTAMPTZ
);
CREATE UNIQUE INDEX payment_transactions_chain_key ON payment_transactions (network_id, chain_tx_key);
CREATE INDEX payment_transactions_payment_idx ON payment_transactions (payment_id);

-- Outgoing sweeps from deposit addresses to the settlement wallet.
CREATE TABLE sweeps (
  sweep_id            BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  project_id          BIGINT NOT NULL REFERENCES projects(project_id) ON DELETE RESTRICT,
  network_id          BIGINT NOT NULL REFERENCES network(network_id),
  asset_network_id    BIGINT NOT NULL REFERENCES asset_network(asset_network_id),
  payment_id          BIGINT REFERENCES payments(payment_id) ON DELETE SET NULL,
  from_address        TEXT NOT NULL,
  to_address          TEXT NOT NULL,
  amount              NUMERIC(38,18) NOT NULL CHECK (amount >= 0),
  network_fee_amount  NUMERIC(38,18) NOT NULL DEFAULT 0 CHECK (network_fee_amount >= 0),
  status              TEXT NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending', 'broadcast', 'confirmed', 'failed')),
  txid                TEXT,
  last_error          TEXT,
  created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX sweeps_project_idx ON sweeps (project_id, created_at DESC);
CREATE INDEX sweeps_payment_idx ON sweeps (payment_id);

-- ---- Webhooks ------------------------------------------------------------------

CREATE TABLE webhook_endpoints (
  webhook_endpoint_id  BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  project_id           BIGINT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  url                  TEXT NOT NULL,
  secret               BYTEA NOT NULL,
  is_enabled           BOOLEAN NOT NULL DEFAULT true,
  created_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX webhook_endpoints_project_idx ON webhook_endpoints (project_id);

-- One row per delivery target. `webhook_endpoint_id` is NULL for a payment's
-- own `ipn_callback_url`.
CREATE TABLE webhook_deliveries (
  webhook_delivery_id  BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  delivery_token       TEXT NOT NULL UNIQUE,
  project_id           BIGINT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  webhook_endpoint_id  BIGINT REFERENCES webhook_endpoints(webhook_endpoint_id) ON DELETE CASCADE,
  url                  TEXT NOT NULL,
  event_type           TEXT NOT NULL,
  event_key            TEXT NOT NULL,
  is_final             BOOLEAN NOT NULL DEFAULT false,
  payload              JSONB NOT NULL,
  status               TEXT NOT NULL DEFAULT 'pending'
                         CHECK (status IN ('pending', 'delivering', 'delivered', 'failed', 'dead')),
  attempt_count        INTEGER NOT NULL DEFAULT 0,
  last_status_code     INTEGER,
  last_error           TEXT,
  next_retry_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
  delivered_at         TIMESTAMPTZ,
  created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX webhook_deliveries_event_key
  ON webhook_deliveries (project_id, COALESCE(webhook_endpoint_id, 0), url, event_key);
CREATE INDEX webhook_deliveries_due_idx ON webhook_deliveries (next_retry_at) WHERE status IN ('pending', 'failed');
CREATE INDEX webhook_deliveries_project_idx ON webhook_deliveries (project_id, created_at DESC);

-- ---- Hot wallets ---------------------------------------------------------------

-- Keys that pay network fees for sweeps and send payouts. The private key is
-- encrypted to the install's age recipient; only the worker can decrypt it.
CREATE TABLE hot_wallets (
  hot_wallet_id  BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  family         TEXT NOT NULL CHECK (family IN ('evm', 'solana', 'tron', 'ton')),
  label          TEXT NOT NULL,
  address        TEXT NOT NULL,
  key_age        BYTEA NOT NULL,
  created_by     BIGINT REFERENCES users(user_id) ON DELETE SET NULL,
  created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX hot_wallets_address_key ON hot_wallets (family, address);

CREATE TABLE project_hot_wallets (
  project_id     BIGINT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  family         TEXT NOT NULL,
  hot_wallet_id  BIGINT NOT NULL REFERENCES hot_wallets(hot_wallet_id) ON DELETE RESTRICT,
  PRIMARY KEY (project_id, family)
);
