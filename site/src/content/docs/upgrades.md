---
title: Upgrades
description: Update Turmite to a new release, with a database dump before each upgrade.
---

Turmite releases are on [GitHub](https://github.com/loruorg/turmite/releases). The image is `ghcr.io/loruorg/turmite`, tagged with each version. When update checks are on, admins see a banner in the dashboard when a new release exists. See [Update checks](#update-checks).

## Before you upgrade

1. Read the release notes in the [changelog](/docs/changelog/). Look for steps that the release needs.
2. If the passphrase is not stored in `secrets/`, be ready to type it after the upgrade. See [Run without a stored passphrase](/docs/install/#run-without-a-stored-passphrase).

Payouts in `signing` or `initiated` need no action. The worker stores each signed transaction before it broadcasts it, and it continues after a restart.

## Upgrade with the install script

Run this on the server:

```bash
curl -fsSL https://turmite.com/install.sh | sudo sh -s -- --upgrade
```

Add `--version X.Y.Z` to choose the version. Without it, the script uses the latest release. The **Updates** page in the dashboard shows the same command.

The script does these steps in `/opt/turmite`:

1. It pulls the new image.
2. It dumps the database to `backups/turmite-<UTC time>-<old version>.sql.gz`. If the dump fails, it stops and changes nothing.
3. It copies `docker-compose.yml` to `backups/docker-compose-<UTC time>.yml`.
4. It writes the `docker-compose.yml`, `backup.sh` and `restore.sh` of the new version.
5. It keeps your `Caddyfile`. If the new version has a different one, it writes it as `Caddyfile.new`. Compare the two files and merge what you need.
6. It sets `TURMITE_VERSION` in `.env`.
7. It runs `docker compose up -d --remove-orphans`. Migrations run when the services start.
8. It waits up to 2 minutes until the `api` is healthy. If the `api` does not get healthy, it prints the last log lines of the `api` and the `worker`.
9. It watches the `worker` container for 15 seconds. If the worker stops or restarts, the script prints its last log lines and the commands to start it, and exits with an error. Without a stored passphrase, the new worker container has no passphrase and stops. Start it with the passphrase as the script shows.

To read the worker log later:

```bash
cd /opt/turmite
sudo docker compose ps
sudo docker compose logs --since 10m worker
```

`backup.sh` deletes dumps that are older than its `--keep-days` value, the upgrade dumps too. Copy an upgrade dump to another place if you want to keep it longer.

## Upgrade by hand

1. Take a dump: `sudo /opt/turmite/backup.sh`.
2. Set the new version in `/opt/turmite/.env`, for example `TURMITE_VERSION=0.2.0`. Docker Compose reads the image tag from this variable.
3. Pull the image and restart the services:

   ```bash
   cd /opt/turmite
   sudo docker compose pull
   sudo docker compose up -d
   ```

4. Check the version:

   ```bash
   sudo docker compose exec api turmite version
   ```

This keeps your `docker-compose.yml`. If the release notes change the compose file, use the install script.

## Run migrations by hand

Every `turmite serve` process runs the migrations at start, in each role. The migration step holds a PostgreSQL advisory lock, so two processes that start at the same time do not apply a migration twice. To run them as a separate step:

```bash
cd /opt/turmite
sudo docker compose run --rm api migrate
```

The command prints `migrations applied` and exits.

## Upgrade from source

```bash
git pull
cargo build --release
./target/release/turmite migrate
```

Then restart the `api` and `worker` processes with the new binary.

## Roll back

Migrations have no down steps. An older version does not start on a database that a newer version migrated. The dump from before the upgrade is the only way back:

1. Set the old version in `.env`: `TURMITE_VERSION=<old version>`.
2. Put back the old compose file:

   ```bash
   cd /opt/turmite
   sudo cp backups/docker-compose-<UTC time>.yml docker-compose.yml
   ```

3. Restore the dump from before the upgrade. The script stops the `api` and the `worker`, loads the dump and starts them again with the old version:

   ```bash
   sudo ./restore.sh backups/turmite-<UTC time>-<old version>.sql.gz
   ```

4. Start the rest of the stack: `sudo docker compose up -d`.

Records from after the dump are lost. Compare recent orders with the chain, as described in [After a restore](/docs/backups/#after-a-restore).

## Update checks

A minute after the worker starts, and then once a day, it reads the latest release and the published security advisories of Turmite from the GitHub API. The request sends `User-Agent: turmite/<version>` and no other data. A worker that restarts does not ask again when the last check is less than an hour old.

Root and admin users see the results:

- A banner when a newer release exists. **Dismiss** hides it until the next release.
- A red banner for each advisory that affects the running version. It stays until the server runs a fixed version.
- The **Server → Updates** page: the running version, the latest release, the last check, the advisories and a **Check now** button.

When SMTP is set up, Turmite emails each critical or high advisory that affects the running version to all root and admin users, one time per advisory. Development builds (`-dev` versions) get no advisories.

Checks are on by default. To turn them off, clear **Check for updates every day** on the **Updates** page. This needs a two-factor code. With checks off, Turmite shows no banners. Then watch the [releases](https://github.com/loruorg/turmite/releases) and the [advisories](https://github.com/loruorg/turmite/security/advisories) on GitHub yourself.
