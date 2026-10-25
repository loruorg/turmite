#!/usr/bin/env bash
# Verify connectivity to every chain endpoint Turmite uses.
# Usage: deploy/testnets/verify.sh [--public]
#   (default checks local docker nodes; --public also probes shared testnets)
set -uo pipefail

PUBLIC=0
[[ "${1:-}" == "--public" ]] && PUBLIC=1

# Optional API keys from deploy/testnets/.env.
ENV_FILE="$(dirname "$0")/.env"
# shellcheck disable=SC1090
[[ -f "$ENV_FILE" ]] && { set -a; . "$ENV_FILE"; set +a; }

pass() { printf '  \033[32mOK\033[0m   %s\n' "$1"; }
fail() { printf '  \033[31mDOWN\033[0m %s  (%s)\n' "$1" "$2"; }

jsonrpc() { # url, post-data, [auth user:pass], [header]
  local url="$1" data="$2" auth="${3:-}" hdr="${4:-}"
  curl -s --max-time 8 ${auth:+--user "$auth"} ${hdr:+-H "$hdr"} \
    -H 'content-type: application/json' --data-binary "$data" "$url"
}

echo "== Local docker nodes =="

out=$(jsonrpc "http://127.0.0.1:18443/" '{"jsonrpc":"1.0","id":"v","method":"getblockchaininfo","params":[]}' "devuser:devpass")
echo "$out" | grep -q '"chain"' && pass "bitcoind regtest (18443)" || fail "bitcoind regtest (18443)" "${out:-no response}"

out=$(curl -s --max-time 8 http://127.0.0.1:18081/get_info)
echo "$out" | grep -q '"status"' && pass "monerod regtest (18081)" || fail "monerod regtest (18081)" "${out:-no response}"

for port in 18083 18084; do
  out=$(jsonrpc "http://127.0.0.1:$port/json_rpc" '{"jsonrpc":"2.0","id":"v","method":"get_version"}')
  echo "$out" | grep -q '"result"' && pass "monero-wallet-rpc ($port)" || fail "monero-wallet-rpc ($port)" "${out:-no response}"
done

out=$(jsonrpc "http://127.0.0.1:8545" '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}')
echo "$out" | grep -q '"0x7a69"' && pass "anvil Ethereum (8545, 31337)" || fail "anvil Ethereum (8545)" "${out:-no response}"

out=$(jsonrpc "http://127.0.0.1:8546" '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}')
echo "$out" | grep -q '"0x61"' && pass "anvil BSC (8546, 97)" || fail "anvil BSC (8546)" "${out:-no response}"

out=$(docker exec turmite-postgres pg_isready -U turmite -d postgres 2>/dev/null)
echo "$out" | grep -q 'accepting connections' && pass "postgres (5433)" || fail "postgres (5433)" "not ready"

# Optional local nodes (only if their profile is up)
out=$(jsonrpc "http://127.0.0.1:8899" '{"jsonrpc":"2.0","id":1,"method":"getHealth"}')
[[ -n "$out" ]] && { echo "$out" | grep -q '"result":"ok"' && pass "solana-test-validator (8899)" || fail "solana-test-validator (8899)" "$out"; }

out=$(curl -s --max-time 8 "http://127.0.0.1:8081/getMasterchainInfo")
[[ -n "$out" ]] && { echo "$out" | grep -q '"ok":true' && pass "ton-http-api for public testnet (8081)" || fail "ton-http-api (8081)" "$out"; }

out=$(curl -s --max-time 8 "http://127.0.0.1:8082/getMasterchainInfo")
[[ -n "$out" ]] && { echo "$out" | grep -q '"ok":true' && pass "ton local chain API (8082)" || fail "ton local chain API (8082)" "$out"; }

out=$(curl -s --max-time 8 -X POST "http://127.0.0.1:9090/wallet/getnowblock")
[[ -n "$out" ]] && { echo "$out" | grep -q 'block_header\|blockID' && pass "tron TRE (9090)" || fail "tron TRE (9090)" "$out"; }

# Profile `public`: signet node + stagenet wallet-rpcs (only if up)
out=$(jsonrpc "http://127.0.0.1:38332/" '{"jsonrpc":"1.0","id":"v","method":"getblockchaininfo","params":[]}' "devuser:devpass")
[[ -n "$out" ]] && { echo "$out" | grep -q '"signet"' && pass "bitcoind signet (38332)" || fail "bitcoind signet (38332)" "$out"; }

for port in 38083 38084; do
  out=$(jsonrpc "http://127.0.0.1:$port/json_rpc" '{"jsonrpc":"2.0","id":"v","method":"get_version"}')
  [[ -n "$out" ]] && { echo "$out" | grep -q '"result"' && pass "monero stagenet wallet-rpc ($port)" || fail "monero stagenet wallet-rpc ($port)" "$out"; }
done

if [[ "$PUBLIC" == "1" ]]; then
  echo "== Public testnets =="

  out=$(jsonrpc "https://ethereum-sepolia-rpc.publicnode.com" '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}')
  echo "$out" | grep -q '"0xaa36a7"' && pass "Ethereum Sepolia (11155111)" || fail "Ethereum Sepolia" "${out:-no response}"

  out=$(jsonrpc "https://data-seed-prebsc-1-s1.bnbchain.org:8545" '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}')
  echo "$out" | grep -q '"0x61"' && pass "BNB Smart Chain testnet (97)" || fail "BSC testnet" "${out:-no response}"

  out=$(jsonrpc "https://api.devnet.solana.com" '{"jsonrpc":"2.0","id":1,"method":"getHealth"}')
  echo "$out" | grep -q '"result":"ok"' && pass "Solana Devnet" || fail "Solana Devnet" "${out:-no response}"

  out=$(curl -s --max-time 10 -X POST "https://nile.trongrid.io/wallet/getnowblock" ${TURMITE_TRON_API_KEY:+-H "TRON-PRO-API-KEY: $TURMITE_TRON_API_KEY"})
  echo "$out" | grep -q 'block_header\|blockID' && pass "Tron Nile testnet" || fail "Tron Nile" "${out:-no response}"

  out=$(curl -s --max-time 10 "https://testnet.toncenter.com/api/v2/getMasterchainInfo" ${TURMITE_TON_API_KEY:+-H "X-API-Key: $TURMITE_TON_API_KEY"})
  echo "$out" | grep -q '"ok":true' && pass "TON testnet (toncenter)" || fail "TON testnet" "${out:-no response}"
fi
