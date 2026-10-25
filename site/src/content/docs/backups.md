---
title: Backups and restore
description: Back up the database, the .env file and the secrets folder, keep a copy off the server, and test a restore.
sidebarTitle: Backups and restore
---

A Turmite install has three items to back up. Your cold wallet seeds are a fourth item, which you keep yourself.

## What to back up

| Item | Location | If you lose it |
|---|---|---|
| Database | Docker volume `pgdata` of the `postgres` service | Payment history, settings, users, encrypted hot keys and the encrypted deposit seed. Funds on chain stay where they are |
| `.env` | `/opt/turmite/.env` | `TURMITE_SECRET`. TOTP secrets, IPN and webhook secrets, node URLs, Monero view keys and exchange API keys become unreadable |
| `secrets/` | `/opt/turmite/secrets/` (`hot-identity.age`, `hot_passphrase`) | The hot wallet identity. The worker cannot decrypt hot keys or the deposit seed, and it does not start |
| Hot wallet passphrase | `secrets/hot_passphrase`, or only with you | The same as losing `secrets/` |
| Cold wallet seeds | Offline, with you | Everything in your cold wallets. Turmite cannot help |

Tron, Solana and TON deposits stay at their deposit addresses until the worker sweeps them. The keys for these addresses come from the deposit seed. Turmite creates the seed on the first start and stores it in the database, encrypted with age to `TURMITE_HOT_RECIPIENT`. To recover unswept funds after a loss, you need the database, `secrets/` and the passphrase from the same install.

## Daily database dump

The install script puts `backup.sh` in `/opt/turmite`. It dumps the database to `backups/turmite-<UTC time>.sql.gz`, prints the file name, and deletes dumps that are older than 14 days.

1. Test it:

   ```bash
   sudo /opt/turmite/backup.sh
   ```

2. Open the crontab of root with `sudo crontab -e`.
3. Add this line. It runs the dump at 03:00 each night:

   ```text
   0 3 * * * /opt/turmite/backup.sh
   ```

Options: `--keep-days N` keeps the dumps for `N` days. `--dir DIR` uses another install directory. The dumps also contain `DROP` statements, so `restore.sh` can load them into a database that has data.

## Copy the dumps off the server

A backup on the same disk does not survive the loss of the server. Copy the dumps to another place each night. This example uses [restic](https://restic.net) with an S3-compatible bucket:

```bash
sudo apt install restic
export RESTIC_REPOSITORY=s3:https://s3.example.com/turmite-backups
export RESTIC_PASSWORD_FILE=/root/.restic-pass
restic init
restic backup /opt/turmite/backups
restic forget --keep-daily 14 --keep-monthly 6 --prune
```

Run the `restic backup` and `restic forget` lines from root's crontab after the dump. Keep the restic password outside the server too.

The dump contains encrypted hot keys and customer data such as order IDs. Encrypt it at rest. restic does this for you.

## Back up .env and secrets

Copy `/opt/turmite/.env` and `/opt/turmite/secrets/` to encrypted storage that is not on the server. Do this after the install and again after each change.

```bash
sudo tar -czf turmite-config-$(date +%F).tar.gz -C /opt/turmite .env secrets
```

Keep the hot wallet passphrase in a password manager, apart from `secrets/`.

Provider snapshots help you restore the whole server quickly. They do not replace the off-site dumps.

## Restore

Use the dump, `.env` and `secrets/` from the same install. `restore.sh` loads a dump in one transaction: if a statement fails, it rolls back the restore. It stops the `api` and the `worker` during the restore, and starts them again after it.

On the same server:

```bash
sudo /opt/turmite/restore.sh /opt/turmite/backups/turmite-20261001-030000.sql.gz
```

The script asks for confirmation. `--yes` skips the question.

On a new server:

1. Install the files without starting the stack. This also installs Docker. The script makes a new `.env` and `secrets/`, which you replace in the next step:

   ```bash
   curl -fsSL https://turmite.com/install.sh | sudo sh -s -- --site-url https://pay.example.com --yes --no-start
   ```

2. Put back `.env` and `secrets/` from your backup:

   ```bash
   sudo tar -xzf turmite-config-2026-10-01.tar.gz -C /opt/turmite
   ```

3. Copy the dump to the server, then load it. PostgreSQL starts for the first time with the database password from your `.env`:

   ```bash
   sudo /opt/turmite/restore.sh --yes /root/turmite-20261001-030000.sql.gz
   ```

4. Start the stack: `cd /opt/turmite && sudo docker compose up -d`.
5. Check the result:
   - `curl -s https://pay.example.com/healthz` prints `ok`;
   - you can sign in to the dashboard;
   - each network on **Server → Networks** shows a recent block;
   - the worker log shows no errors about the hot wallet identity.

If DNS points to the old server, change the records to the new IP address.

The install script pulls the latest image, but your `.env` keeps the old `TURMITE_VERSION`. The stack runs that version. To run the latest version, [upgrade](/docs/upgrades/) after the restore.

## After a restore

Records from after the dump are not in the database. The worker watches only the payments that are in the database, so it does not see funds for payments that were made after the dump. These funds are still on chain:

- Bitcoin and Monero funds are in your wallets.
- EVM funds are in deposit contracts that can only send to your cold wallet.
- Tron, Solana and TON funds wait at their deposit addresses.

Compare recent orders in your shop with the chain and your wallets. Ask customers who paid in that time to contact you.

The address counters also come back from the dump. New payments can then get deposit addresses that Turmite gave out after the dump, and two payments share one address. Before the stack takes payments, move the counters on with an amount larger than the number of payments made after the dump:

```bash
cd /opt/turmite
sudo docker compose stop api worker
sudo docker compose run --rm api counters raise --by 1000
sudo docker compose start api worker
```

The command adds the amount to each counter and deletes the free pool addresses. On Bitcoin, the skipped addresses are a gap in your wallet: set the gap limit of your wallet above the amount. See [`turmite counters raise`](/docs/cli/#turmite-counters-raise).

The same change in SQL, for example in `psql` (`sudo docker compose exec postgres psql -U turmite -d turmite`):

```sql
BEGIN;
UPDATE network_address_counter SET next_index = next_index + 1000;
UPDATE family_address_counter SET next_index = next_index + 1000;
UPDATE project_wallets SET next_index = next_index + 1000;
DELETE FROM deposit_addresses WHERE project_id IS NULL AND payment_id IS NULL;
COMMIT;
```

Monero subaddresses do not need this step while you keep the wallet files on the wallet RPC host: the wallet RPC counts the subaddresses in each file.

## Test a restore

Restore to a spare server at least once before you go live. Repeat the test after large upgrades.
