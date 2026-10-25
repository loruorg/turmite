-- Bitcoin and Monero watch-only wallets.

-- A deposit that left the mempool without a block (replaced, evicted, double-spent) is
-- observed with confirmations = -1. It then has dropped_at set and stops counting toward
-- the payment. When it comes back, dropped_at is cleared.
ALTER TABLE payment_transactions DROP CONSTRAINT payment_transactions_confirmations_check;
ALTER TABLE payment_transactions ADD CONSTRAINT payment_transactions_confirmations_check CHECK (confirmations >= -1);
ALTER TABLE payment_transactions ADD COLUMN dropped_at TIMESTAMPTZ;

CREATE FUNCTION payment_tx_dropped() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.confirmations < 0 THEN
    NEW.confirmations := -1;
    IF TG_OP = 'UPDATE' THEN
      NEW.dropped_at := COALESCE(OLD.dropped_at, now());
    ELSE
      NEW.dropped_at := now();
    END IF;
  ELSE
    NEW.dropped_at := NULL;
  END IF;
  RETURN NEW;
END
$$;

CREATE TRIGGER payment_tx_dropped BEFORE INSERT OR UPDATE OF confirmations ON payment_transactions
  FOR EACH ROW EXECUTE FUNCTION payment_tx_dropped();

-- First receive addresses of a project's Bitcoin account key, shown so the operator can
-- compare them with the wallet.
ALTER TABLE project_wallets ADD COLUMN check_addresses TEXT[];
