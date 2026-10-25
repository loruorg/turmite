-- EVM deposit contracts: salt counter, deposit addresses, watch state, hot-wallet transactions.

-- Install-wide index counter per chain family. EVM deposit salts come from it: one salt gives the
-- same deposit address on every EVM chain, so a salt is used once for all networks and projects.
CREATE TABLE family_address_counter (
  family      TEXT PRIMARY KEY,
  next_index  BIGINT NOT NULL DEFAULT 1
);

-- The factory and cold wallet of each EVM deposit address. They stay with the address when the
-- project changes its cold wallet later. `from_block` is the chain tip when the address was made.
CREATE TABLE evm_deposits (
  network_id   BIGINT NOT NULL REFERENCES network(network_id),
  address      TEXT NOT NULL,
  factory      TEXT NOT NULL,
  cold_wallet  TEXT NOT NULL,
  salt_index   BIGINT NOT NULL CHECK (salt_index >= 0),
  from_block   BIGINT NOT NULL,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (network_id, address)
);

-- Watch state of one asset at one deposit address (`asset` = '' for the native coin, else the token
-- contract). Arrivals in blocks up to `final_to` are final; later ones are read again on each pass.
-- Amounts are in base units.
CREATE TABLE evm_scans (
  network_id     BIGINT NOT NULL REFERENCES network(network_id),
  address        TEXT NOT NULL,
  asset          TEXT NOT NULL,
  final_to       BIGINT NOT NULL,
  final_flushed  NUMERIC(78,0) NOT NULL DEFAULT 0,
  arrivals       JSONB NOT NULL DEFAULT '[]',
  updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (network_id, address, asset)
);

-- Transactions that hot wallets send for the deposit contracts. "Deploy now" in the dashboard adds a
-- 'queued' deploy; the worker sends it.
CREATE TABLE evm_txs (
  evm_tx_id     BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  network_id    BIGINT NOT NULL REFERENCES network(network_id),
  project_id    BIGINT REFERENCES projects(project_id) ON DELETE SET NULL,
  kind          TEXT NOT NULL CHECK (kind IN ('deploy', 'flush')),
  factory       TEXT NOT NULL,
  cold_wallet   TEXT NOT NULL,
  from_address  TEXT,
  nonce         BIGINT,
  txid          TEXT,
  salts         INTEGER NOT NULL DEFAULT 0,
  gas_used      BIGINT,
  fee_wei       NUMERIC(78,0),
  status        TEXT NOT NULL CHECK (status IN ('queued', 'broadcast', 'confirmed', 'failed')),
  last_error    TEXT,
  requested_by  BIGINT REFERENCES users(user_id) ON DELETE SET NULL,
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX evm_txs_open_idx ON evm_txs (network_id) WHERE status IN ('queued', 'broadcast');
CREATE INDEX evm_txs_factory_idx ON evm_txs (network_id, factory, evm_tx_id DESC);
CREATE UNIQUE INDEX evm_txs_txid_key ON evm_txs (network_id, txid) WHERE txid IS NOT NULL;
