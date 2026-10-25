#!/bin/sh
# Every test, real-chain tests included, against the local nodes in deploy/testnets.
# Start the nodes first:
#   docker compose -f deploy/testnets/docker-compose.yml --profile all up -d
# TURMITE_TEST_CHAINS=all makes the real-chain tests run. A test that still skips
# (it prints "SKIP:") counts as a failure here.
set -u
cd "$(dirname "$0")/.." || exit 1

log=$(mktemp)
rc=$(mktemp)
trap 'rm -f "$log" "$rc"' EXIT

{
	TURMITE_TEST_CHAINS=${TURMITE_TEST_CHAINS:-all} cargo test -p turmite --features all-chains --no-fail-fast -- --nocapture 2>&1
	echo $? >"$rc"
} | tee "$log"

if [ "$(cat "$rc")" != 0 ]; then
	echo ""
	echo "FAIL: see the test output above."
	exit 1
fi
if grep -q '^SKIP:' "$log"; then
	echo ""
	echo "FAIL: some real-chain tests skipped. Is every node up?"
	grep '^SKIP:' "$log"
	exit 1
fi
echo ""
echo "PASS: every test, real chains included."
