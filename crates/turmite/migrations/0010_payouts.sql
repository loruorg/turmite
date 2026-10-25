-- Payouts from hot wallets: settings, address book, batches, payouts, signed transactions,
-- emailed approval codes.

-- Limits per project, in USD. 0 = no limit, except `auto_approve_usd`: 0 = every payout needs approval.
CREATE TABLE payout_settings (
  project_id        BIGINT PRIMARY KEY REFERENCES projects(project_id) ON DELETE CASCADE,
  min_usd           NUMERIC(20,2) NOT NULL DEFAULT 0 CHECK (min_usd >= 0),
  auto_approve_usd  NUMERIC(20,2) NOT NULL DEFAULT 0 CHECK (auto_approve_usd >= 0),
  hourly_limit_usd  NUMERIC(20,2) NOT NULL DEFAULT 0 CHECK (hourly_limit_usd >= 0),
  daily_limit_usd   NUMERIC(20,2) NOT NULL DEFAULT 0 CHECK (daily_limit_usd >= 0),
  updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Address book. API payouts can only go to these addresses.
CREATE TABLE payout_addresses (
  payout_address_id  BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  project_id         BIGINT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
  network_id         BIGINT NOT NULL REFERENCES network(network_id),
  label              TEXT NOT NULL CHECK (LENGTH(label) BETWEEN 1 AND 80),
  address            TEXT NOT NULL,
  extra_id           TEXT,
  created_by         BIGINT REFERENCES users(user_id) ON DELETE SET NULL,
  created_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX payout_addresses_key ON payout_addresses (project_id, network_id, address, COALESCE(extra_id, ''));

CREATE TABLE payout_batches (
  batch_id          BIGINT GENERATED ALWAYS AS IDENTITY (START WITH 5000000001) PRIMARY KEY,
  project_id        BIGINT NOT NULL REFERENCES projects(project_id) ON DELETE RESTRICT,
  source            TEXT NOT NULL CHECK (source IN ('api', 'dashboard')),
  created_by        BIGINT REFERENCES users(user_id) ON DELETE SET NULL,
  api_key_id        BIGINT REFERENCES api_keys(api_key_id) ON DELETE SET NULL,
  created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX payout_batches_project_idx ON payout_batches (project_id, created_at DESC);

CREATE TABLE payouts (
  payout_id           BIGINT GENERATED ALWAYS AS IDENTITY (START WITH 5000200001) PRIMARY KEY,
  batch_id            BIGINT NOT NULL REFERENCES payout_batches(batch_id) ON DELETE RESTRICT,
  project_id          BIGINT NOT NULL REFERENCES projects(project_id) ON DELETE RESTRICT,
  asset_network_id    BIGINT NOT NULL REFERENCES asset_network(asset_network_id),
  network_id          BIGINT NOT NULL REFERENCES network(network_id),
  address             TEXT NOT NULL,
  extra_id            TEXT,
  amount              NUMERIC(38,18) NOT NULL CHECK (amount > 0),
  -- USD value when the payout was created; NULL when no rate was available.
  amount_usd          NUMERIC(38,8),
  status              TEXT NOT NULL CHECK (status IN ('pending-approval', 'pending-otp-verification', 'pending',
                        'signing', 'initiated', 'sent', 'failed', 'rejected', 'cancelled', 'needs-review')),
  -- Counts status changes; makes each webhook event key unique.
  status_seq          INTEGER NOT NULL DEFAULT 0,
  -- Why the payout waits for a dashboard approval.
  approval_reason     TEXT,
  description         TEXT,
  unique_external_id  TEXT,
  ipn_callback_url    TEXT,
  hot_wallet_id       BIGINT REFERENCES hot_wallets(hot_wallet_id) ON DELETE RESTRICT,
  from_address        TEXT,
  txid                TEXT,
  -- Network fee in the native coin of the network.
  fee                 NUMERIC(38,18),
  confirmations       INTEGER NOT NULL DEFAULT 0 CHECK (confirmations >= 0),
  error               TEXT,
  resolution_note     TEXT,
  created_by          BIGINT REFERENCES users(user_id) ON DELETE SET NULL,
  verified_by         BIGINT REFERENCES users(user_id) ON DELETE SET NULL,
  verified_at         TIMESTAMPTZ,
  approved_by         BIGINT REFERENCES users(user_id) ON DELETE SET NULL,
  -- When the payout was cleared to send. Hourly and daily limits count from this time.
  approved_at         TIMESTAMPTZ,
  -- First time a node accepted the transaction.
  requested_at        TIMESTAMPTZ,
  -- First time every node refused the stored transaction.
  refused_at          TIMESTAMPTZ,
  -- Next time the signer looks at this payout.
  next_check_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
  created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX payouts_idempotency_key ON payouts (project_id, unique_external_id) WHERE unique_external_id IS NOT NULL;
CREATE INDEX payouts_project_idx ON payouts (project_id, created_at DESC);
CREATE INDEX payouts_batch_idx ON payouts (batch_id);
CREATE INDEX payouts_active_idx ON payouts (next_check_at) WHERE status IN ('pending', 'signing', 'initiated');
CREATE INDEX payouts_limits_idx ON payouts (project_id, approved_at) WHERE approved_at IS NOT NULL;

-- Every signed transaction of a payout, stored before it is broadcast. EVM fee bumps add rows with the
-- same nonce; Solana, Tron and TON add a row when an expired transaction is signed again.
CREATE TABLE payout_attempts (
  payout_attempt_id  BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  payout_id          BIGINT NOT NULL REFERENCES payouts(payout_id) ON DELETE CASCADE,
  network_id         BIGINT NOT NULL REFERENCES network(network_id),
  from_address       TEXT NOT NULL,
  txid               TEXT NOT NULL,
  raw_tx             TEXT NOT NULL,
  -- EVM nonce or TON seqno.
  nonce              BIGINT,
  -- Fee parameters (EVM) or validity (Solana blockhash, Tron and TON expiry).
  params             JSONB NOT NULL DEFAULT '{}',
  valid_until        TIMESTAMPTZ,
  broadcast_at       TIMESTAMPTZ,
  last_error         TEXT,
  created_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX payout_attempts_payout_idx ON payout_attempts (payout_id, payout_attempt_id);
CREATE INDEX payout_attempts_nonce_idx ON payout_attempts (network_id, from_address, nonce) WHERE nonce IS NOT NULL;

-- Emailed approval code: one per payout, bound to the admin who clicked Approve. Stored as a keyed hash.
CREATE TABLE payout_approval_codes (
  payout_id   BIGINT PRIMARY KEY REFERENCES payouts(payout_id) ON DELETE CASCADE,
  user_id     BIGINT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
  code_hash   TEXT NOT NULL,
  attempts    INTEGER NOT NULL DEFAULT 0,
  expires_at  TIMESTAMPTZ NOT NULL,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
