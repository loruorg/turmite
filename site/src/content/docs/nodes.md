---
title: Networks and nodes
description: Add RPC nodes for each network, set confirmations and tokens, and keep a second node for failover.
---

Turmite reads each chain through RPC nodes. You add the nodes in the dashboard. Turmite stores the node URLs in the database, sealed with `TURMITE_SECRET`. The networks, tickers and contracts are in [Tickers and networks](/docs/networks/).

## The Networks page

Open **Server → Networks**. You need the `root` or `admin` role. Each network has a card with:

- the enabled tickers, the highest block that a node reported, and its age;
- a badge: **Connected**, **Not connected**, **No node** or **Off**, and **No new block** when the last block is old;
- a table of nodes with the columns **Node**, **URL**, **Status**, **Height** and **Checked**. **In use** marks the node that the worker reads. The status is **OK**, **Error**, **Unknown** or **Off**, with the number of blocks the node is behind and its last error;
- the **Add node** form;
- the **Confirmations** field and the **Network on** check box;
- **Assets**: the tickers of the network, and a form to add a token.

**No new block** shows after 60 minutes without a new block on Bitcoin, 20 minutes on Monero and 5 minutes on the other networks.

## Node types

| Family | Node | Example URL |
|---|---|---|
| Bitcoin | Bitcoin Core (`bitcoind`) JSON-RPC with wallet support. Put the RPC user and password in the URL | `http://user:password@bitcoind:8332` |
| Monero | `monero-wallet-rpc`, started with `--wallet-dir` and `--disable-rpc-login` | `http://monero-wallet-rpc:18083` |
| EVM | Ethereum JSON-RPC | `https://eth-mainnet.g.alchemy.com/v2/KEY` |
| Tron | Tron full node HTTP API | `https://api.trongrid.io` |
| Solana | Solana JSON-RPC | `https://api.mainnet-beta.solana.com` |
| TON | toncenter HTTP API v2 | `https://toncenter.com/api/v2` |

The URL must start with `http://` or `https://`.

API keys:

- A key in the URL path works for every family.
- Tron and TON: put the key in an `apikey` or `api_key` query parameter. Turmite removes it from the URL and sends it as a header: `TRON-PRO-API-KEY` on Tron, `X-API-Key` on TON.
- Bitcoin: Turmite sends the user and password of the URL as HTTP basic authentication.

Bitcoin: Turmite makes one watch-only wallet per project on bitcoind, named `turmite-p<project id>`. Use one bitcoind per Turmite install, because two installs use the same wallet names. A bitcoind that you add later watches each project's addresses from the moment Turmite gives it the key. It does not find deposits that arrived before that moment.

Monero: Turmite makes one view-only wallet file per project in the wallet directory. A wallet RPC has one open wallet at a time, and Turmite opens each project's file in turn. Give Turmite its own wallet RPC. The wallet files have no password, so keep the wallet RPC on a private network.

Tron: a node with the `/v1/accounts` history API, such as TronGrid, lists each transfer. A plain full node gives only balances. Then Turmite does not see a top-up that arrives after a sweep.

## Add a node

1. Open **Server → Networks**.
2. In the card of the network, enter the URL in **Add node**.
3. Optional: enter a **Label**, up to 40 characters. Without a label, Turmite uses the host name.
4. Enter a fresh code in **Two-factor code**.
5. Click **Add**.

Turmite saves the node, then connects to the network again. The message tells whether the network is connected. When the connection fails, the node stays in the list with its error. To delete a node, enter a two-factor code next to it and click **Remove**.

Turmite checks the node when it connects:

| Family | Check |
|---|---|
| EVM | The node's chain ID must be the network's chain ID |
| Solana | The genesis hash must be mainnet-beta on a mainnet install and devnet on a testnet install. On a testnet install, a local URL is also accepted |
| Bitcoin | When you save an account key, the key type must match the node's chain |
| Monero | When Turmite makes a wallet, the wallet RPC must accept the address for its network |
| Tron | The genesis block (block 0) must be mainnet on a mainnet install. A testnet install accepts Nile, Shasta and a local chain, but not mainnet |
| TON | The masterchain zerostate must be mainnet on a mainnet install. A testnet install accepts the public testnet and a local chain, but not mainnet |

Node URLs often contain API keys. The dashboard and the audit log show each URL without the user name, the password and the query. A path longer than 12 characters shows as `/redacted`.

## Failover

Add at least two nodes for each network on mainnet. Use two providers, or your own node and one provider.

- The order of the nodes is the order in which you added them. Turmite uses the first node that connects.
- The worker checks every node every 30 seconds. The `api` process checks every 120 seconds.
- A node fails a check when it does not answer in 15 seconds, or when it is too many blocks behind the best node of its network.
- When the node in use fails two checks in a row, Turmite moves the network to the next healthy node.
- When a node earlier in the order passes two checks in a row, Turmite moves the network back to it.
- Each move writes a `node.failover` entry in the audit log.
- Turmite connects again when you add or remove a node, or save a network.

| Network | A node fails when it is more blocks behind than |
|---|---|
| Bitcoin | 2 |
| Monero | 3 |
| Ethereum | 10 |
| Base, Polygon | 60 |
| BNB Smart Chain | 160 |
| Solana | 300 |
| Tron, TON | 40 |

A Monero wallet RPC gives a height only while a project's wallet is open. Without a height, a connected wallet RPC passes the check.

Public endpoints have rate limits. Use them for tests only.

## Confirmations

Each network has a confirmation count. A deposit stays in `confirming` until it has that many confirmations. Then the payment becomes `confirmed`, and `finished` when the deposits cover the amount due. The defaults are in [Tickers and networks](/docs/networks/#confirmations).

1. In the card of the network, change **Confirmations**. The count is 0 to 10000.
2. To lower the count, enter a fresh code in **Two-factor code**. Raising the count needs no code.
3. Click **Save**.

The same form turns the network on or off with **Network on**.

- Keep the default counts, or raise them. For large amounts, a higher count gives more protection against chain reorganisations.
- A count of 0 credits a deposit that has no confirmation. Do not use 0 on mainnet.
- On TON, a deposit counts as confirmed when the node lists it. The count has no effect.

The checkout shows the progress as `1 of 3`, `2 of 3`. Up to 12 confirmations, it also shows one segment per confirmation.

## Tokens

The contract addresses come from the catalog that is built into Turmite (`crates/turmite/src/chains/catalog.rs`). Turmite uses the correct contracts for the install's mode.

Open **Assets** in the card of a network to see its tickers with their contracts and decimals.

- To turn a ticker off or on, change its **On** check box. Enter a fresh code in **Two-factor code** and click **Save assets**. The native coin is in the list too.
- To add a token on an EVM, Tron or Solana network, fill in **Asset**, **Ticker**, **Decimals** and **Contract**. Enter a fresh code and click **Add token**. The ticker has 2 to 20 lower-case letters and digits, and must be new. Decimals are 0 to 30. Each asset can have one contract per network.

Copy the contract address from the token issuer. A wrong address loses payments.

## Trust in nodes

A node cannot move funds. Turmite signs each sweep and payout itself, so a node cannot change where funds go. A node can give wrong data:

- A node that hides transactions delays payments. A second node and the block heights on the Networks page show this.
- A node that reports a false deposit can mark an unpaid order as paid.

Use nodes that you run or pay for. For large orders, check the deposit on a block explorer before you ship.

## When a network falls behind

If the Networks page shows **No new block**, or a node shows that it is behind:

1. Look at the node status and errors on the Networks page.
2. Read the worker log for errors about the network:

   ```bash
   cd /opt/turmite
   docker compose logs --since 1h worker | grep -i error
   ```

3. Add a second node, or replace the node that fails.

The worker checks each open payment every 5 seconds. When a node answers again, the next check finds the deposits. Turmite watches a payment until one day after its payment window ends. A deposit that arrives later does not count. See [Troubleshooting](/docs/troubleshooting/#payments).
