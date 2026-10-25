#!/bin/sh
# Install or upgrade Turmite on an Ubuntu or Debian server.
#
#   curl -fsSL https://turmite.com/install.sh | sudo sh -s -- --site-url https://pay.example.com
#   curl -fsSL https://turmite.com/install.sh | sudo sh -s -- --upgrade
#
# The script installs Docker Engine and the Compose plugin from Docker's apt repository
# when they are missing, writes the stack to /opt/turmite, runs `turmite init` (it asks
# for the hot-wallet passphrase) and starts PostgreSQL, the API, the worker and Caddy.
set -eu

REPO=loruorg/turmite
IMAGE=loruorg/turmite

dir=/opt/turmite
mode=
site_url=
version=
yes=0
start=1
upgrade=0
local_dir=

usage() {
	cat <<'EOF'
Usage: install.sh [options]

  --site-url URL   Public URL of the server, for example https://pay.example.com
  --mode MODE      mainnet or testnet (asks; default with --yes: mainnet)
  --version X.Y.Z  Turmite version (default: the latest release)
  --dir DIR        Install directory (default: /opt/turmite)
  --yes            Do not ask; install Docker when it is missing
  --no-start       Write the files but do not start the stack
  --upgrade        Back up the database, then move an existing install to the new version
  --local DIR      Take the files from DIR (a checkout of the repository), not from GitHub
  -h, --help       Show this help
EOF
}

say() { printf '%s\n' "$@"; }
die() {
	printf 'install.sh: %s\n' "$*" >&2
	exit 1
}

need_arg() { [ $# -ge 2 ] && [ -n "$2" ] || die "$1 needs a value"; }

while [ $# -gt 0 ]; do
	case $1 in
	--site-url) need_arg "$@"; site_url=$2; shift 2 ;;
	--mode) need_arg "$@"; mode=$2; shift 2 ;;
	--version) need_arg "$@"; version=$2; shift 2 ;;
	--dir) need_arg "$@"; dir=$2; shift 2 ;;
	--local) need_arg "$@"; local_dir=$2; shift 2 ;;
	--yes | -y) yes=1; shift ;;
	--no-start) start=0; shift ;;
	--upgrade) upgrade=1; shift ;;
	-h | --help) usage; exit 0 ;;
	*) usage >&2; die "unknown option: $1" ;;
	esac
done

case $mode in "" | mainnet | testnet) ;; *) die "--mode must be mainnet or testnet" ;; esac

# Prompts read the terminal, also when the script comes through a pipe.
has_tty=0
if (: </dev/tty) 2>/dev/null; then has_tty=1; fi

ask() {
	# ask "question" -> 0 for yes
	[ "$yes" = 1 ] && return 0
	[ "$has_tty" = 1 ] || die "$1 Run again with --yes to accept."
	printf '%s [y/N] ' "$1" >/dev/tty
	read -r answer </dev/tty || answer=
	case $answer in y | Y | yes | YES) return 0 ;; *) return 1 ;; esac
}

[ "$(id -u)" = 0 ] || die "run as root (sudo sh install.sh ...)"
command -v curl >/dev/null 2>&1 || die "curl is missing: apt-get install -y curl"

# ---- Docker --------------------------------------------------------------------

install_docker() {
	[ -r /etc/os-release ] || die "cannot read /etc/os-release; install Docker yourself"
	# shellcheck disable=SC1091
	. /etc/os-release
	case ${ID:-} in
	ubuntu) distro=ubuntu; suite=${UBUNTU_CODENAME:-${VERSION_CODENAME:-}} ;;
	debian) distro=debian; suite=${VERSION_CODENAME:-} ;;
	*) die "this script supports Ubuntu and Debian; install Docker Engine and the Compose plugin yourself" ;;
	esac
	[ -n "$suite" ] || die "cannot find the release code name in /etc/os-release"
	say "Installing Docker Engine from download.docker.com ($distro $suite)."
	export DEBIAN_FRONTEND=noninteractive
	apt-get update -q
	apt-get install -y -q ca-certificates curl
	install -m 0755 -d /etc/apt/keyrings
	curl -fsSL "https://download.docker.com/linux/$distro/gpg" -o /etc/apt/keyrings/docker.asc
	chmod a+r /etc/apt/keyrings/docker.asc
	cat >/etc/apt/sources.list.d/docker.sources <<EOF
Types: deb
URIs: https://download.docker.com/linux/$distro
Suites: $suite
Components: stable
Signed-By: /etc/apt/keyrings/docker.asc
EOF
	apt-get update -q
	apt-get install -y -q docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
	if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
		systemctl enable --now docker
	fi
}

if ! command -v docker >/dev/null 2>&1 || ! docker compose version >/dev/null 2>&1; then
	ask "Docker Engine or its Compose plugin is missing. Install them from Docker's apt repository?" ||
		die "Docker is needed. Install it and run this script again."
	install_docker
fi
docker info >/dev/null 2>&1 || die "the Docker daemon does not answer (systemctl start docker)"

# ---- Version and files ---------------------------------------------------------

latest_release() {
	curl -fsSL -H 'Accept: application/vnd.github+json' "https://api.github.com/repos/$REPO/releases/latest" |
		sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1
}

if [ -z "$version" ]; then
	version=$(latest_release) || true
	[ -n "$version" ] || die "cannot find the latest release; pass --version"
fi
version=${version#v}
case $version in
*[!0-9A-Za-z.-]* | "") die "bad version: $version" ;;
esac

# fetch PATH DEST: a file of the repository at the release tag.
fetch() {
	if [ -n "$local_dir" ]; then
		cp "$local_dir/$1" "$2"
	else
		curl -fsSL "https://raw.githubusercontent.com/$REPO/v$version/$1" -o "$2"
	fi
}

fetch_scripts() {
	for s in backup.sh restore.sh; do
		fetch "scripts/$s" "$dir/$s"
		chmod 755 "$dir/$s"
	done
}

compose() { (cd "$dir" && docker compose "$@"); }

pull_image() {
	if ! docker image inspect "$IMAGE:$version" >/dev/null 2>&1; then
		docker pull "$IMAGE:$version"
	fi
}

wait_healthy() {
	i=0
	while [ $i -lt 60 ]; do
		if compose exec -T api turmite healthcheck >/dev/null 2>&1; then return 0; fi
		i=$((i + 1))
		sleep 2
	done
	compose logs --tail 50 api worker >&2 || true
	die "the API did not become healthy; see the logs above"
}

# The worker opens no port: read the state of its container for 15 seconds.
check_worker() {
	state=
	i=0
	while [ $i -lt 5 ]; do
		sleep 3
		id=$(compose ps -aq worker 2>/dev/null | head -n 1)
		state=
		[ -z "$id" ] || state=$(docker inspect -f '{{.State.Status}} {{.RestartCount}}' "$id" 2>/dev/null || true)
		[ "$state" = "running 0" ] || break
		i=$((i + 1))
	done
	[ "$state" = "running 0" ] && return 0
	compose logs --tail 20 worker >&2 || true
	say "" "The worker does not run (${state:-no container}). Until it runs, Turmite does not confirm, sweep or notify payments."
	if grep -q '^TURMITE_HOT_PASSPHRASE_FILE=.' "$dir/.env"; then
		say "Read the log above, correct the cause, then start the worker:" "  cd $dir && sudo docker compose up -d worker"
	else
		say "This install does not store the hot-wallet passphrase. Start the worker with the passphrase:" \
			"  sudo -i" \
			"  cd $dir" \
			"  read -rs TURMITE_HOT_PASSPHRASE && export TURMITE_HOT_PASSPHRASE" \
			"  docker compose up -d worker" \
			"  unset TURMITE_HOT_PASSPHRASE"
	fi
	return 1
}

set_env() {
	# set_env KEY VALUE in $dir/.env
	if grep -q "^$1=" "$dir/.env"; then
		sed -i "s|^$1=.*|$1=$2|" "$dir/.env"
	else
		printf '%s=%s\n' "$1" "$2" >>"$dir/.env"
	fi
}

# ---- Upgrade -------------------------------------------------------------------

if [ "$upgrade" = 1 ]; then
	[ -f "$dir/.env" ] || die "$dir/.env is missing; this is not an install directory"
	current=$(sed -n 's/^TURMITE_VERSION=//p' "$dir/.env" | head -n 1)
	say "Upgrading Turmite ${current:-?} to $version in $dir."
	pull_image
	mkdir -p "$dir/backups"
	chmod 700 "$dir/backups"
	stamp=$(date -u +%Y%m%d-%H%M%S)
	backup="$dir/backups/turmite-$stamp-${current:-unknown}.sql"
	say "Backing up the database to $backup.gz."
	compose up -d --wait postgres
	(umask 077 && compose exec -T postgres pg_dump -U turmite -d turmite --clean --if-exists >"$backup.part") ||
		die "the database backup failed; nothing was changed"
	gzip -9 "$backup.part"
	mv "$backup.part.gz" "$backup.gz"
	backup=$backup.gz
	cp "$dir/docker-compose.yml" "$dir/backups/docker-compose-$stamp.yml"
	fetch deploy/docker-compose.yml "$dir/docker-compose.yml.new"
	mv "$dir/docker-compose.yml.new" "$dir/docker-compose.yml"
	fetch_scripts
	fetch deploy/Caddyfile "$dir/Caddyfile.new"
	if cmp -s "$dir/Caddyfile" "$dir/Caddyfile.new"; then
		rm -f "$dir/Caddyfile.new"
	else
		say "The Caddyfile of $version differs from yours. Compare $dir/Caddyfile.new and merge what you need."
	fi
	set_env TURMITE_VERSION "$version"
	compose up -d --remove-orphans
	wait_healthy
	check_worker || die "the API runs $version, but the worker does not. The backup is $backup."
	say "Turmite $version is running. The backup is $backup."
	exit 0
fi

# ---- Install -------------------------------------------------------------------

[ ! -f "$dir/.env" ] || die "$dir/.env exists. Use --upgrade, or remove $dir to start again."

if [ -z "$site_url" ]; then
	[ "$has_tty" = 1 ] && [ "$yes" = 0 ] || die "pass --site-url https://pay.example.com"
	printf 'Public URL of this server (for example https://pay.example.com): ' >/dev/tty
	read -r site_url </dev/tty || true
	[ -n "$site_url" ] || die "a site URL is needed"
fi
case $site_url in http://* | https://*) ;; *) die "--site-url must start with https:// or http://" ;; esac

if [ -z "$mode" ]; then
	mode=mainnet
	if [ "$has_tty" = 1 ] && [ "$yes" = 0 ]; then
		printf 'Mode: mainnet (real funds) or testnet [mainnet]: ' >/dev/tty
		read -r answer </dev/tty || answer=
		mode=${answer:-mainnet}
	fi
	case $mode in mainnet | testnet) ;; *) die "the mode must be mainnet or testnet" ;; esac
fi

say "Installing Turmite $version ($mode) in $dir for $site_url."
mkdir -p "$dir"
chmod 755 "$dir"
fetch deploy/docker-compose.yml "$dir/docker-compose.yml"
fetch deploy/Caddyfile "$dir/Caddyfile"
fetch_scripts
pull_image

# init asks for the passphrase on the terminal. Without one, give it in
# TURMITE_HOT_PASSPHRASE or let --yes make a random one (stored in secrets/).
if [ -n "${TURMITE_HOT_PASSPHRASE:-}" ]; then
	prompt=0
elif [ "$has_tty" = 1 ] && [ "$yes" = 0 ]; then
	prompt=1
elif [ "$yes" = 1 ]; then
	TURMITE_HOT_PASSPHRASE=$(od -An -N24 -tx1 /dev/urandom | tr -d ' \n')
	export TURMITE_HOT_PASSPHRASE
	prompt=0
	say "Made a random hot-wallet passphrase. It is in $dir/secrets/hot_passphrase."
else
	die "set TURMITE_HOT_PASSPHRASE, run in a terminal, or pass --yes"
fi

if [ "$prompt" = 1 ]; then
	docker run --rm -it --user 0:0 -v "$dir:/data" "$IMAGE:$version" \
		init --dir /data --mode "$mode" --site-url "$site_url" </dev/tty
else
	docker run --rm --user 0:0 -e TURMITE_HOT_PASSPHRASE -v "$dir:/data" "$IMAGE:$version" \
		init --dir /data --mode "$mode" --site-url "$site_url" </dev/null
fi
set_env TURMITE_VERSION "$version"

if [ "$start" = 0 ]; then
	say "Files are in $dir. Start the stack with: cd $dir && docker compose up -d"
	exit 0
fi

compose up -d
wait_healthy
check_worker || die "the API runs, but the worker does not"

cat <<EOF

Turmite $version is running at $site_url

Next steps:
  1. Open $site_url/dashboard and create the root account.
  2. Turn on two-factor authentication on the account page.
  3. Add RPC nodes for your networks in Server > Networks.
  4. Keep an offline copy of $dir/.env and $dir/secrets/. Without them the hot keys are lost.
  5. Back up the database daily: add "0 3 * * * $dir/backup.sh" to the crontab of root.

Upgrade later with:
  curl -fsSL https://turmite.com/install.sh | sudo sh -s -- --upgrade
EOF
