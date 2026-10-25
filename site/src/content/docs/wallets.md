---
title: Wallets per chain
description: Connect a wallet for each network, and check the first deposit address before you take payments.
sidebarTitle: Wallets per chain
---

Each project has one wallet setup per network. Open **Wallets** in the project to set them. You need the `project_admin` role or a global admin role. Each save needs a fresh two-factor code and writes an audit log entry.

The Wallets page shows only the networks that are on. A card shows **No node** when its network has no connected node.

The checkout offers a coin when its network and its token are on and the project has a wallet for the network. Connect a node for the network too. Without a node, the customer gets an error when they select the coin.

To stop taking payments on one network, click **Stop taking payments on `Network`** in its card. Click **Take payments on `Network` again** to start again. This switch needs no two-factor code.

## Custody per chain

| Network | You give Turmite | Deposit address | Who can move the funds |
|---|---|---|---|
| Bitcoin | Account public key (`xpub` or `zpub`) | Receive address of your account | Only you. Turmite has no Bitcoin key and does not sweep |
| Monero | Primary address, private view key, restore height | Subaddress of your wallet | Only you. The view key cannot spend. Turmite does not sweep |
| Ethereum, Base, Polygon, BNB Smart Chain | One cold wallet address | Deposit contract per payment (CREATE2) | The contract can only send to your cold wallet |
| Tron, Solana, TON | Your address (TON: optional memo) | Key per payment from the install's deposit seed | The worker, until it sweeps the funds to your address |

Use a hardware wallet or a multisig as the cold wallet. Turmite never needs its private key.

## Bitcoin

Turmite watches the receive chain of a native SegWit (BIP84) account. Each payment gets the next receive address of that account. The coins go straight to your wallet.

1. In your wallet, export the account public key of a BIP84 account. On mainnet it starts with `zpub` or `xpub`. On Signet and the other test networks it starts with `vpub` or `tpub`.
2. In Turmite, open **Wallets**. In the Bitcoin card, paste the key in **Account key**.
3. Enter a fresh code in **Two-factor code** and click **Save**.
4. Compare **Address 0**, **Address 1** and **Address 2** with the first three receive addresses in your wallet (`m/84'/0'/0'/0/0` to `/2` on mainnet). They must be the same.

If the addresses are different, do not take payments with this key. Turmite converts `zpub` to `xpub` and `vpub` to `tpub`. An `xpub` can also come from a legacy (BIP44) account. Turmite then shows native SegWit addresses that your wallet does not have.

Turmite refuses these keys:

- private keys (`xprv`, `zprv`, `tprv`, `vprv` and the other private formats);
- wrapped SegWit keys (`ypub`, `upub`);
- multisig keys (`Ypub`, `Zpub`, `Upub`, `Vpub`);
- a key for another network than the install's mode.

With a connected node, Turmite also checks the key against the node's chain. It makes a watch-only wallet `turmite-p<project id>` on bitcoind and shows the three addresses. Without a node, Turmite saves the key and shows no addresses. Connect a node, then save the key again.

Each payment uses a new address. Turmite never uses an address twice, also after you change the key. Expired payments leave unused addresses. Many wallets stop at 20 unused addresses in a row. Use a wallet only for this shop, and set its gap limit above the number of open payments.

A deposit can leave the mempool without a block, for example when the sender replaces it. Turmite then stops counting it toward the payment. The payment page in the dashboard shows **Dropped** for it. If the deposit comes back, it counts again.

## Monero

Turmite makes a view-only wallet from your primary address and private view key. Each payment gets a new subaddress of account 0 of that wallet. The coins go straight to your wallet.

| Field | Value |
|---|---|
| **Primary address** | The main address of the wallet, 95 characters. It starts with `4` on mainnet and `5` on Stagenet. Integrated addresses and subaddresses are refused |
| **Private view key** | 64 hexadecimal characters, from the keys page of your wallet. Never enter the spend key |
| **Restore height** | Optional. Leave it empty to start at the current block |

1. Open **Wallets** and fill in the Monero card.
2. Enter a fresh code in **Two-factor code** and click **Save**.

Turmite checks that the view key belongs to the address before it sends either to the node. Payments go to new subaddresses, so blocks before the setup are not necessary. When the wallet RPC cannot give the current height, enter the restore height. A low height makes the first scan slow.

Turmite seals the view key with `TURMITE_SECRET` and never shows it again. Each save needs the view key again.

The Monero node is a `monero-wallet-rpc`. Turmite makes the wallet file `turmite-p<project id>-<hash>` in its wallet directory. The hash comes from the address and the restore height, so a new address or height gives a new file. The files have no password. See [Networks and nodes](/docs/nodes/#node-types).

A Monero payment needs 10 confirmations, because Monero locks received outputs for 10 blocks.

## Ethereum, Base, Polygon and BNB Smart Chain

One cold wallet serves the four EVM networks. Turmite computes a factory contract address from it. The factory has the same address on each EVM network. Each payment gets a deposit address that the factory computes with CREATE2. The deposit contract can only send funds to the cold wallet in the factory. A full compromise of the server cannot change this.

1. Open **Wallets**. In the **EVM networks** card, enter **Cold wallet address**.
2. Enter a fresh code in **Two-factor code** and click **Save**. The card shows the factory address.
3. Assign an EVM hot wallet to the project. See [Hot wallets](#hot-wallets).
4. On each network that shows **Not deployed**, enter a fresh code and click **Deploy now**. The worker sends the deployment from the project's EVM hot wallet within a minute.

Turmite checks the address with the EIP-55 rules. An address in mixed case must have a correct checksum. All lower case and all upper case are accepted. Turmite stores the address in EIP-55 form.

You can also skip **Deploy now**:

- The worker deploys the factory before the first sweep on a network.
- Anyone can deploy it. Open **Deploy the factory from your own wallet** and send the transaction (**To** and **Data**) with no value. The result is always the same factory.

A deposit address can receive funds before its contract exists. The first sweep deploys the contract.

### Sweeps

The worker sweeps in batches. One `flush` transaction moves the funds of many payments of one project and one factory to the cold wallet. The EVM hot wallet pays the gas. It cannot send the funds anywhere else.

A batch goes out when one of these is true:

- the gas is at most 2 % of the batch value;
- the batch is worth 1000 USD or more;
- the batch has 150 payments;
- the oldest payment in the batch waited 6 hours.

A batch never goes out while the gas is more than 20 % of its value. The funds then wait at the deposit addresses. Without USD prices, only a batch of native coins can be checked. Other batches wait.

Turmite uses EIP-1559 fees: twice the base fee plus the node's priority fee, up to 1000 gwei. Above that cap, sends wait. When the hot wallet has too little native coin for the gas, the batch waits. When a token refuses the transfer, Turmite tries that payment again after one hour.

The card lists the recent factory deployments and sweeps, with their status and fee. It shows **No hot wallet** when the project has no EVM hot wallet.

If your cold wallet is a smart contract wallet, such as a multisig, it must exist at the same address on each network that you use. A multisig on Ethereum does not exist on Base until you deploy it there. Funds that go to an address with no wallet behind it are lost.

To change the cold wallet, save the new address. It has a new factory. Payments made before the change keep their factory: their funds still go to the old cold wallet.

## Tron, Solana and TON

On these chains, a deposit address cannot be locked to a destination. Turmite derives a key for each payment from the install's deposit seed. The worker sweeps the funds to your address after the deposit has the required confirmations. Until the sweep, the worker can move those funds.

1. Open **Wallets** and find the Tron, Solana or TON card.
2. Enter your **Address**. On TON, enter a **Memo** if the destination needs one. Turmite sends it as the text comment of each sweep.
3. Enter a fresh code in **Two-factor code** and click **Save**.

Turmite checks the address before it saves it:

| Network | With a connected node | Without a node |
|---|---|---|
| Tron | A valid `T` address | A `T` address with a correct checksum |
| Solana | A public key that is not a token account or a token mint. Enter the wallet address that owns your token accounts | A base58 public key of 32 bytes |
| TON | A valid raw or user-friendly address | 48 characters, or a raw address with `:` |

Turmite creates the deposit seed on the first start. It stores the seed in the database, encrypted with age to `TURMITE_HOT_RECIPIENT`. Only the worker can decrypt it, with the hot-wallet identity and the passphrase. Back up the database and `secrets/` together. Without both, the keys of unswept deposits are lost.

The `api` process does not have the deposit seed. The worker keeps 25 free deposit addresses for each of these networks and adds more every 30 seconds. When no free address is left, the merchant API answers `503` with `Retry-After: 30`.

Sweep fees per asset:

| Asset | Who pays the fee |
|---|---|
| TRX | The deposit. Turmite keeps back the bandwidth fee, or the activation fee when your address is new on chain |
| USDT on Tron | The Tron hot wallet. It sends the deposit the TRX for energy and bandwidth, 5 % above the estimate. Then the deposit sends the tokens |
| SOL | The deposit. It pays the fee and keeps nothing |
| USDC and USDT on Solana | The Solana hot wallet. It pays the fee and the rent of your token account if that account does not exist. Turmite closes the empty deposit token account, and its rent goes back to the hot wallet |
| TON | The deposit. It sends its whole balance and the fees come out of it. The first sweep also deploys the deposit wallet (v4r2). Balances below 0.01 TON are not swept |

TON deposit addresses are wallets that are not yet deployed. Turmite shows them in the non-bounceable form.

## Hot wallets

A hot wallet pays network fees for sweeps and sends payouts. Global admins manage them in **Server → Hot wallets**. Bitcoin and Monero have no hot wallet.

| Family | Sweeps need the hot wallet for |
|---|---|
| EVM | Gas for the factory deployment and for each batch |
| Tron | USDT sweeps |
| Solana | Token sweeps |
| TON | No sweeps. Payouts only |

- **Make a hot wallet** generates a key. **Import a key** stores a key that you have.
- **Assign to a project** gives a project one hot wallet per family. One hot wallet can serve many projects.
- Each of these actions needs a fresh two-factor code.

Keep a small balance in each hot wallet: about one week of fees and expected payouts. A server compromise can spend this balance. It cannot reach your cold wallets. See [Payouts](/docs/payouts/#how-hot-wallets-work).

## Test each wallet

Before you go live, send a small real payment on each network. Check that:

1. the payment reaches `finished`;
2. the funds arrive in your wallet (EVM, Tron, Solana and TON: after the sweep);
3. your shop receives the webhook.

See the [Go-live checklist](/docs/go-live/).
