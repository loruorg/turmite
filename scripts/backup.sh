#!/bin/sh
# Dump the Turmite database to <dir>/backups/turmite-<UTC time>.sql.gz and delete
# dumps older than --keep-days. Run it on the server, for example from cron:
#   0 3 * * * /opt/turmite/backup.sh --keep-days 14
# A dump alone cannot be restored on a new server: keep .env and secrets/ with it.
set -eu

dir=$(cd "$(dirname "$0")" && pwd)
keep_days=14

while [ $# -gt 0 ]; do
	case $1 in
	--dir) dir=$2; shift 2 ;;
	--keep-days) keep_days=$2; shift 2 ;;
	-h | --help)
		echo "Usage: backup.sh [--dir /opt/turmite] [--keep-days 14]"
		exit 0
		;;
	*) echo "backup.sh: unknown option: $1" >&2; exit 1 ;;
	esac
done

case $keep_days in '' | *[!0-9]*) echo "backup.sh: --keep-days must be a number" >&2; exit 1 ;; esac
[ -f "$dir/docker-compose.yml" ] || { echo "backup.sh: no docker-compose.yml in $dir (use --dir)" >&2; exit 1; }

umask 077
mkdir -p "$dir/backups"
out="$dir/backups/turmite-$(date -u +%Y%m%d-%H%M%S).sql"
if ! (cd "$dir" && docker compose exec -T postgres pg_dump -U turmite -d turmite --clean --if-exists) >"$out.part"; then
	rm -f "$out.part"
	echo "backup.sh: pg_dump failed" >&2
	exit 1
fi
gzip -9 "$out.part"
mv "$out.part.gz" "$out.gz"
echo "$out.gz"

find "$dir/backups" -maxdepth 1 -name 'turmite-*.sql.gz' -mtime +"$keep_days" -exec rm -f {} +
