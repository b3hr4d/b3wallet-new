# 0008: Three kinds of account, and one of each from the start

Status: proposed, 2026-10-22. Behrad asked in the thread for one Internet
Computer account kind in place of ICP and ICRC, for EVM through the EVM RPC
canister's own chains only, and for one account of each kind from the start,
with adding an account moved out of the way. The details below are
recommendations for him to confirm.

**Decision: an account is a key or a subaccount, and holds every asset of its
kind. An Internet Computer account holds ICP and the ICRC-1 tokens the wallet
tracks, a Bitcoin account holds bitcoin, and an EVM account holds the native
token of each EVM chain the wallet tracks. Every wallet has one account of
each kind from the start.**

## Why

An ICP account and an ICRC account are already the same thing under another
name. The wallet sends ICP with `icrc1_transfer`, and the index of an account
counts per chain, so the first ICP account and the first account of any ICRC
ledger are both subaccount 0 of the wallet's principal: one address shown as
two accounts. Each new token needs a new account, though its address is one
the wallet already has.

The same holds for EVM: an address comes from a key, not a chain, so one EVM
account receives on every EVM chain.

## The three kinds

| Kind | Account `n` is | Holds |
| --- | --- | --- |
| Internet Computer | ICRC-1 subaccount `n` of the wallet's principal ([0002](0002-subaccounts.md)) | ICP and every tracked ICRC-1 ledger |
| Bitcoin | The threshold ECDSA key of `wallet` / `bitcoin-<network>` / `n`, as now | Bitcoin of the wallet's network |
| EVM | The threshold ECDSA key of `wallet` / `evm` / `n` | The native token of every tracked EVM chain |

An Internet Computer account also shows its ICP account ID, the 64-digit hex
form that exchanges often ask for, so it can receive ICP from them.

## Tracked assets

| Option | What the accounts show | For | Against |
| --- | --- | --- | --- |
| **A. One list for the wallet** | Every account of a kind shows every tracked asset of that kind | Add a token once; a new account shows it at once | An account shows a zero for a token it never held |
| B. A list per account | Each account its own tokens | Only what an account holds | The same token added again for each account |

Recommended: A. The list starts with ICP and Ethereum. Owners add ICRC-1
ledgers and switch EVM chains on or off without approval, as they add
accounts now: tracking moves nothing. Daily limits stay per asset, as they
are now.

## The default accounts

The wallet makes Internet Computer account 0, Bitcoin account 0 and, from the
EVM step, EVM account 0, labelled "Main". A timer set in `init` and
`post_upgrade` fetches the threshold keys and makes whichever is missing, so
a new wallet has them before its claim and an existing one gains them on
upgrade. The start page lists them, and adding an account becomes a small
link under the list.

The wallet does not know its Bitcoin network today; the page picks regtest
when it runs locally.

| Option | How the wallet learns it | For | Against |
| --- | --- | --- | --- |
| **A. An install setting** | `InitArgs` gains `bitcoin_network`; `Upgrade` takes an optional record with it, which `variant { Upgrade }` still decodes as | Exact; the factory says mainnet and `try-local.sh` says regtest | A field in the install arguments |
| B. Mainnet always | No setting | Nothing to set | A local wallet's default Bitcoin account cannot read a balance |

Recommended: A. A wallet upgraded without one takes the network of its first
Bitcoin account, and mainnet if it has none; `try-local.sh` gains an
`--upgrade` that passes regtest.

## EVM in v1

Only the chains the EVM RPC canister (`7hfb6-caaaa-aaaar-qadga-cai`) has its
own providers for: Ethereum (chain id 1), Sepolia (11155111), Arbitrum One
(42161), Base (8453) and Optimism (10). There is no custom chain or RPC URL,
so no single RPC is trusted and there is no URL setting to put behind an
approval. Each read asks several providers and takes an answer only when
they agree: 2 of 3 is the recommendation. A transaction's chain id is part of
its signature, so it cannot be replayed on another chain. The EVM RPC
canister quotes each call's cost (`eth_*CyclesCost`), and the EVM step
records those measured costs; a balance read on every tracked chain is one
call per chain, which is why only Ethereum is tracked from the start.

## The interface

- `Account` gains `kind : AccountKind`, `variant { InternetComputer; Bitcoin :
  BitcoinNetwork; Evm }`, in place of `chain`; `AddAccountArgs` likewise.
- `Chain` stays and names where an asset lives: `Icp`, `Icrc`, `Bitcoin`,
  `Evm`. Daily limits keep it.
- `balance` takes the asset as well as the account, and `TransferRequest`
  gains `asset : opt Chain`. It is optional so that actions stored before the
  change still decode; absent, it is the asset the account held then.
- New methods list and change the tracked assets.

The pages ship with the wallet; the MCP server in `mcp/` changes in the same
step. Other clients that read `Account.chain` need updating.

Upgrading an existing wallet merges its ICP and ICRC accounts that share an
index into one Internet Computer account. The oldest keeps its id and label,
the others' ids lead to it, so audit entries and open transfers still find
their account, and their ledgers become tracked.

## Not in this decision

- ERC-20 tokens on EVM chains, such as USDC.
- Custom EVM chains and RPC URLs.
- ICRC-2 approvals, which let another canister spend from an account and so
  need the policy like a transfer.
- Sending ICP to an account ID, which needs the ICP ledger's older `transfer`
  method; this step only shows an account's own ID.

## Steps

16. Internet Computer accounts, tracked ICRC-1 ledgers, the default Internet
    Computer and Bitcoin accounts, the Bitcoin network setting and the
    upgrade that merges accounts.
17. EVM accounts through the EVM RPC canister, with the default EVM account.
