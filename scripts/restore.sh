#!/bin/sh
# Restore a dump made by backup.sh (or install.sh --upgrade) into the Turmite database.
#   /opt/turmite/restore.sh /opt/turmite/backups/turmite-20260101-030000.sql.gz
# The API and the worker stop during the restore. The server must have the .env and
# secrets/ of the server that made the dump, or sealed data and hot keys stay unreadable.
set -eu

dir=$(cd "$(dirname "$0")" && pwd)
yes=0
file=

while [ $# -gt 0 ]; do
	case $1 in
	--dir) dir=$2; shift 2 ;;
	--yes | -y) yes=1; shift ;;
	-h | --help)
		echo "Usage: restore.sh [--dir /opt/turmite] [--yes] FILE.sql.gz"
		exit 0
		;;
	-*) echo "restore.sh: unknown option: $1" >&2; exit 1 ;;
	*) file=$1; shift ;;
	esac
done

[ -n "$file" ] || { echo "restore.sh: give the dump file" >&2; exit 1; }
[ -r "$file" ] || { echo "restore.sh: cannot read $file" >&2; exit 1; }
[ -f "$dir/docker-compose.yml" ] || { echo "restore.sh: no docker-compose.yml in $dir (use --dir)" >&2; exit 1; }
gzip -t "$file" || { echo "restore.sh: $file is not a valid gzip file" >&2; exit 1; }

if [ "$yes" = 0 ]; then
	printf 'This replaces the database in %s with %s. Continue? [y/N] ' "$dir" "$file"
	read -r answer || answer=
	case $answer in y | Y | yes | YES) ;; *) echo "Stopped."; exit 1 ;; esac
fi

compose() { (cd "$dir" && docker compose "$@"); }

compose stop api worker
compose up -d --wait postgres
if gzip -dc "$file" | compose exec -T postgres psql -U turmite -d turmite -v ON_ERROR_STOP=1 --single-transaction -q >/dev/null; then
	status=0
else
	status=1
	echo "restore.sh: the restore failed and was rolled back" >&2
fi
compose up -d api worker
[ "$status" = 0 ] && echo "Restored $file."
exit "$status"
