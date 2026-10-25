-- Count the status changes of each payment. Webhook event keys include the count, so a payment
-- that comes back to an earlier status gets a new webhook.
ALTER TABLE payments ADD COLUMN status_seq INTEGER NOT NULL DEFAULT 0;

CREATE FUNCTION payments_count_status() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.status IS DISTINCT FROM OLD.status THEN
    NEW.status_seq := OLD.status_seq + 1;
  END IF;
  RETURN NEW;
END
$$;

CREATE TRIGGER payments_count_status BEFORE UPDATE OF status ON payments
  FOR EACH ROW EXECUTE FUNCTION payments_count_status();
