# Turmite testnet stack

PostgreSQL and a local node for each chain: Bitcoin, Monero, Ethereum, BNB Smart Chain, Solana, Tron and TON. All images run on arm64 and amd64. Profile `public` adds clients for Bitcoin signet and Monero stagenet.

## Start

```bash
docker compose -f deploy/testnets/docker-compose.yml up -d                  # Postgres, BTC, XMR, ETH, BSC
docker compose -f deploy/testnets/docker-compose.yml --profile all up -d    # + Solana, Tron, TON
docker compose -f deploy/testnets/docker-compose.yml --profile public up -d # + signet node, stagenet wallet-rpcs

deploy/testnets/verify.sh            # local nodes
deploy/testnets/verify.sh --public   # + public testnets
```

Optional settings (stagenet node, Tron and TON API keys) go in `deploy/testnets/.env`. See `.env.example`.

## Tests

`cargo test` needs only PostgreSQL. The real-chain tests run when `TURMITE_TEST_CHAINS` names their chain (`bitcoin,monero`) or is `all`:

```bash
scripts/e2e.sh   # every test, every chain; a skipped chain test is a failure
```

## Endpoints

| Chain | Mode | Endpoint | Notes |
|---|---|---|---|
| Postgres | local | `postgres://turmite:turmite@127.0.0.1:5433` | host port 5433 |
| Bitcoin | regtest | `http://127.0.0.1:18443` (devuser/devpass) | Bitcoin Core 31.1 |
| Bitcoin | signet | `http://127.0.0.1:38332` | profile `public`, pruned, must sync first |
| Monero | regtest | daemon `:18081`, engine wallet-rpc `:18083`, payer wallet-rpc `:18084` | wallet-rpcs on loopback only (no login) |
| Monero | stagenet | engine wallet-rpc `:38083`, payer `:38084` | profile `public`, public node `TURMITE_XMR_STAGENET_DAEMON` |
| Ethereum | local | anvil `http://127.0.0.1:8545` (31337) | `test…junk` mnemonic, 10000 ETH each |
| Ethereum | public | Sepolia `https://ethereum-sepolia-rpc.publicnode.com` | |
| BSC | local | anvil `http://127.0.0.1:8546` (97) | same EVM client as Ethereum |
| BSC | public | `https://data-seed-prebsc-1-s1.bnbchain.org:8545` (97) | |
| Solana | local | `http://127.0.0.1:8899` | profile `solana` |
| Solana | public | Devnet `https://api.devnet.solana.com` | |
| Tron | local | TRE `http://127.0.0.1:9090` | profile `tron`; prefunded accounts in its logs |
| Tron | public | Nile `https://nile.trongrid.io` | API key recommended |
| TON | local | `http://127.0.0.1:8082` | profile `ton-local` (MyLocalTon); faucet wallet below |
| TON | public | `https://testnet.toncenter.com/api/v2` (or local API `:8081`, profile `ton`) | API key recommended |

TON local faucet: masterchain wallet v3r2, id 42, `-1:22f53b7d9aba2cef44755f7078b01614cd4dde2388a1729c2c386cf8f9898afe`, secret `a51e8fb6f0fae3834bf430f5012589d319e7b3b3303ceb82c816b762fccf2d05` (public test key).

## Faucets

Nile `nileex.io/join/getJoinPage`, signet `alt.signetfaucet.com`, stagenet `stagenet-faucet.xmr-tw.org`, Sepolia and BSC `faucet.zalalena.com`, TON `acton wallet airdrop`, Devnet `faucet.solana.com` or `solana airdrop`.

## Funding cheatsheet

```bash
# Bitcoin regtest
b() { docker exec turmite-bitcoind bitcoin-cli -regtest -rpcuser=devuser -rpcpassword=devpass -rpcport=18443 "$@"; }
b -named createwallet wallet_name=test; b loadwallet test
ADDR=$(b -rpcwallet=test getnewaddress); b generatetoaddress 101 "$ADDR"
b -rpcwallet=test sendtoaddress <deposit> 1.0 && b generatetoaddress 1 "$ADDR"

# Monero regtest: mine to the payer wallet (coinbase unlocks after 60 blocks, payments after 10)
curl -s http://127.0.0.1:18081/json_rpc -d '{"jsonrpc":"2.0","id":"0","method":"generateblocks","params":{"amount_of_blocks":80,"wallet_address":"<payer addr>"}}'

# Ethereum / BSC anvil (account 0 key)
docker exec turmite-anvil cast send <to> --value 1ether --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
docker exec turmite-anvil-bsc cast send <to> --value 1ether --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80

# Solana local
curl -s http://127.0.0.1:8899 -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"requestAirdrop","params":["<pubkey>",2000000000]}'
```

## Gotchas

- Monero shows a send between subaddresses of one wallet as "out" only. The customer side needs its own wallet-rpc (`:18084`).
- Monero outputs unlock after 10 blocks.
- TON deposit addresses are undeployed wallets: they are shown non-bounceable (`0Q…` on testnet) and deployed by the first sweep.
- `ton-http-api-cpp` dies with SIGILL on Apple Silicon; the stack uses the Python `toncenter/ton-http-api` (port 8081 inside the container).
- The genesis TON node needs its fixed IP (subnet `10.231.42.0/24`).
- Profile `ton` needs the testnet config: `curl -sL https://ton-blockchain.github.io/testnet-global.config.json -o deploy/testnets/ton/testnet.json`.
- A new stagenet wallet needs some minutes for its first scan through a public node.
- Public testnets reset and rate-limit: back off on 429.

## Teardown

```bash
docker compose -f deploy/testnets/docker-compose.yml --profile all --profile public down      # keep volumes
docker compose -f deploy/testnets/docker-compose.yml --profile all --profile public down -v   # wipe chain state
```
