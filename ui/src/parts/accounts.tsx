import { isPrincipal, principal, type Principal } from "@candid-core/schema"
import { useMutation, type UseQueryResult } from "@tanstack/react-query"
import { useState, type ReactNode } from "react"
import type {
  Account,
  AccountKind,
  Balance,
  BitcoinNetwork,
  Chain,
  Member,
} from "../generated/wallet.ts"
import { useWallet } from "../ic/canister.ts"
import { useToken } from "../ic/ledger.ts"
import { failureText, shortText } from "../logic/describe.ts"
import {
  amountParts,
  chainKey,
  chainText,
  evmChainsOf,
  ICP_ONLY,
  kindText,
  type Token,
} from "../logic/tokens.ts"
import { CopyButton } from "./copy.tsx"
import { ArrowIcon, CrossIcon, PlusIcon, RefreshIcon } from "./icons.tsx"

/** The read of what one account holds of one asset. */
export type BalanceRead = { account: Account; asset: Chain; read: UseQueryResult<Balance> }

/** An amount to show large: grouped digits, a quieter fraction, the symbol. */
export function Amount({ amount, token }: { amount: bigint; token: Token }) {
  const { whole, fraction, symbol } = amountParts(amount, token)
  return (
    <span className="amount">
      <span className="amount-whole">{whole}</span>
      <span className="amount-fraction">{fraction}</span>
      <span className="amount-symbol">{symbol}</span>
    </span>
  )
}

/**
 * What the wallet holds, a total per asset over its accounts, once every
 * balance of that asset is read, with how it splits between them.
 */
export function Holdings({
  accounts,
  balances,
}: {
  accounts: UseQueryResult<Account[]>
  balances: BalanceRead[]
}) {
  if (accounts.isError) return <p className="error">{failureText(accounts.error)}</p>
  if (accounts.data === undefined) {
    return (
      <div className="holdings">
        <div className="holding">
          <span className="skeleton" />
          <span className="skeleton big" />
        </div>
      </div>
    )
  }
  const groups = new Map<string, { asset: Chain; parts: BalanceRead[] }>()
  for (const balance of balances) {
    const key = chainKey(balance.asset)
    const group = groups.get(key) ?? { asset: balance.asset, parts: [] }
    group.parts.push(balance)
    groups.set(key, group)
  }
  return (
    <div className="holdings">
      {[...groups].map(([key, group]) => (
        <Holding key={key} asset={group.asset} parts={group.parts} />
      ))}
    </div>
  )
}

/** One asset's total, under its symbol, and its split between the accounts that hold it. */
function Holding({ asset, parts }: { asset: Chain; parts: BalanceRead[] }) {
  const { token, error } = useToken(asset)
  const amounts = parts.map(({ read }) => read.data?.amount)
  const failed = error !== undefined || parts.some(({ read }) => read.isError)
  const allRead = amounts.every((amount) => amount !== undefined)
  const sum = amounts.reduce<bigint>((total, amount) => total + (amount ?? 0n), 0n)
  const total = token === undefined || !allRead ? undefined : amountParts(sum, token)
  return (
    <div className="holding">
      <p className="holding-label">
        {token ? <strong>{token.symbol}</strong> : <span className="skeleton" />}
        <span>
          {parts.length} account{parts.length === 1 ? "" : "s"}
        </span>
      </p>
      {failed ? (
        <p className="holding-amount quiet">Not read</p>
      ) : total === undefined ? (
        <span className="skeleton big" />
      ) : (
        <p className="holding-amount">
          {total.whole}
          <span className="amount-fraction">{total.fraction}</span>
        </p>
      )}
      {!failed && allRead && parts.length > 1 && sum > 0n && (
        <Split
          parts={parts.map(({ account, read }) => ({
            label: account.label,
            share: Number(((read.data?.amount ?? 0n) * 1000n) / sum) / 10,
          }))}
        />
      )}
    </div>
  )
}

/**
 * How an asset splits between accounts: a bar of a segment each, widest
 * first, and the shares in words. The bar is drawn in its own units, since
 * the CSP allows no style attribute.
 */
function Split({ parts }: { parts: Array<{ label: string; share: number }> }) {
  const sorted = [...parts].sort((a, b) => b.share - a.share)
  const starts = sorted.map((_, index) =>
    sorted.slice(0, index).reduce((sum, part) => sum + part.share, 0),
  )
  return (
    <div className="split">
      <svg viewBox="0 0 100 4" preserveAspectRatio="none" aria-hidden="true">
        {sorted.map((part, index) => (
          <rect
            key={part.label + index}
            className={`split-${Math.min(index, 3)}`}
            x={starts[index]}
            width={Math.max(part.share - 0.6, 0.4)}
            height="4"
          />
        ))}
      </svg>
      <p className="split-legend">
        {sorted.slice(0, 3).map((part, index) => (
          <span key={part.label + index} className={`split-${Math.min(index, 3)}`}>
            {part.label} {part.share < 1 && part.share > 0 ? "<1" : Math.round(part.share)}%
          </span>
        ))}
      </p>
    </div>
  )
}

/**
 * The wallet's accounts with their balances and the tokens they show, for
 * every member; owners also track tokens and add accounts here. Every wallet
 * starts with one account of each kind, so adding one is the last thing on
 * the card.
 */
export function Accounts({
  wallet: id,
  me,
  network,
  accounts,
  assets,
  balances,
}: {
  wallet: Principal
  me: Member
  network: BitcoinNetwork
  accounts: UseQueryResult<Account[]>
  assets: UseQueryResult<Chain[]>
  balances: BalanceRead[]
}) {
  const owner = me.role.tag === "Owner"
  const [adding, setAdding] = useState(false)
  const reading = balances.some(({ read }) => read.isFetching)
  return (
    <section className="card" id="accounts">
      <div className="card-head">
        <h2>Accounts</h2>
        {accounts.data !== undefined && <span className="count">{accounts.data.length}</span>}
        <div className="card-end">
          {balances.length > 0 && (
            <button
              type="button"
              className="ghost small"
              disabled={reading}
              onClick={() => balances.forEach(({ read }) => void read.refetch())}
            >
              <RefreshIcon />
              {reading ? "Reading…" : "Refresh"}
            </button>
          )}
        </div>
      </div>
      {accounts.isError ? (
        <p className="error">{failureText(accounts.error)}</p>
      ) : accounts.data === undefined ? (
        <ul className="accounts">
          <li className="account-outline">
            <span className="avatar skeleton" />
            <span className="skeleton wide" />
          </li>
        </ul>
      ) : (
        <ul className="accounts">
          {accounts.data.map((account) => (
            <AccountItem
              key={account.id}
              account={account}
              reads={balances.filter((balance) => balance.account.id === account.id)}
            />
          ))}
        </ul>
      )}
      <Tokens wallet={id} network={network} assets={assets} owner={owner} />
      {owner &&
        (adding ? (
          <AddAccount wallet={id} network={network} onDone={() => setAdding(false)} />
        ) : (
          <p className="accounts-more">
            <button type="button" className="ghost small" onClick={() => setAdding(true)}>
              <PlusIcon />
              Add another account
            </button>
          </p>
        ))}
    </section>
  )
}

/**
 * One account, as a card: its first asset's balance large, which the wallet
 * asks a ledger, the Bitcoin canister or an EVM chain's providers for, its
 * other tokens, and its address to copy. The card opens the account's own
 * page, which holds the rest.
 */
function AccountItem({ account, reads }: { account: Account; reads: BalanceRead[] }) {
  const [first, ...more] = reads
  const token = useToken(first?.asset)
  const failure = first?.read.isError ? first.read.error : token.error
  let shown: ReactNode
  if (first === undefined) {
    // An EVM account while the wallet tracks no EVM chain: nothing to read.
    shown = <span className="quiet">No chain tracked</span>
  } else if (failure !== undefined && failure !== null) {
    shown = <span className="error">Not read</span>
  } else if (first.read.data === undefined || token.token === undefined) {
    shown = <span className="skeleton" />
  } else {
    shown = <Amount amount={first.read.data.amount} token={token.token} />
  }
  return (
    <li className="account">
      <div className="account-top">
        <KindAvatar kind={account.kind} />
        <p className="account-name">
          <a className="account-link" href={`/account?id=${account.id}`}>
            <strong>{account.label}</strong>
          </a>
          <span>
            {kindText(account.kind)} · {account.index}
          </span>
        </p>
        <ArrowIcon />
      </div>
      <p className="account-balance">{shown}</p>
      {more.length > 0 && (
        <ul className="holds">
          {more.map(({ asset, read }) => (
            <HeldAsset key={chainKey(asset)} asset={asset} read={read} />
          ))}
        </ul>
      )}
      <p className="account-address">
        <code>{shortText(account.address, 10, 6)}</code>
        <CopyButton text={account.address} label={`Copy the address of ${account.label}`} />
      </p>
    </li>
  )
}

/** Another token an account holds, on a line of its own. */
export function HeldAsset({ asset, read }: { asset: Chain; read: UseQueryResult<Balance> }) {
  const { token, error } = useToken(asset)
  let shown: ReactNode
  if (error !== undefined || read.isError) shown = <span className="error">Not read</span>
  else if (token === undefined || read.data === undefined) shown = <span className="skeleton" />
  else shown = <Amount amount={read.data.amount} token={token} />
  return (
    <li className="held">
      <span className="quiet">{token?.symbol ?? chainText(asset)}</span>
      {shown}
    </li>
  )
}

/** What an account receives, over its address. */
export function receivesText(kind: AccountKind): string {
  switch (kind.tag) {
    case "InternetComputer":
      return "Receives ICP and ICRC-1 tokens at"
    case "Bitcoin":
      return "Receives bitcoin at"
    case "Evm":
      return "Receives on each tracked EVM chain at"
  }
}

/** An account by its kind, in plain text: networks are named, never drawn. */
export function KindAvatar({ kind }: { kind: AccountKind }) {
  const text = kind.tag === "InternetComputer" ? "ICP" : kind.tag === "Bitcoin" ? "BTC" : "EVM"
  return (
    <span className="avatar token" aria-hidden="true">
      {text}
    </span>
  )
}

/**
 * The tokens the Internet Computer accounts show, ICP and the ICRC-1 ledgers
 * owners track, and the chains the EVM accounts show. Tracking moves
 * nothing, so an owner changes it at once; the wallet asks a ledger whether
 * it is one before tracking it.
 */
function Tokens({
  wallet: id,
  network,
  assets,
  owner,
}: {
  wallet: Principal
  network: BitcoinNetwork
  assets: UseQueryResult<Chain[]>
  owner: boolean
}) {
  const { client, wallet } = useWallet(id)
  const track = useMutation(
    client.mutationOptions(wallet, "track_asset", {
      invalidates: [
        [wallet, "assets"],
        [wallet, "audit_log"],
      ],
    }),
  )
  const trackChain = useMutation(
    client.mutationOptions(wallet, "track_asset", {
      invalidates: [
        [wallet, "assets"],
        [wallet, "audit_log"],
      ],
    }),
  )
  const untrack = useMutation(
    client.mutationOptions(wallet, "untrack_asset", {
      invalidates: [
        [wallet, "assets"],
        [wallet, "audit_log"],
      ],
    }),
  )
  const [adding, setAdding] = useState(false)
  const [ledger, setLedger] = useState("")
  const [addingChain, setAddingChain] = useState(false)
  const [pickedChain, setChainId] = useState("")
  const typed = ledger.trim()
  const valid = isPrincipal(typed)
  if (assets.isError) return <p className="error">{failureText(assets.error)}</p>
  const tracked = assets.data ?? ICP_ONLY
  const chains = tracked.filter((asset) => asset.tag === "Evm")
  const untracked = evmChainsOf(network).filter(
    (chain) => !chains.some((asset) => asset.value === chain.id),
  )
  const chainId = untracked.some((chain) => String(chain.id) === pickedChain)
    ? pickedChain
    : String(untracked[0]?.id ?? "")
  return (
    <div className="tokens">
      <p className="field-label">Tokens the Internet Computer accounts show</p>
      <div className="chips">
        {tracked
          .filter((asset) => asset.tag !== "Evm")
          .map((asset) => (
            <TokenChip
              key={chainKey(asset)}
              asset={asset}
              busy={untrack.isPending}
              onUntrack={owner && asset.tag === "Icrc" ? () => untrack.mutate(asset) : undefined}
            />
          ))}
        {owner && !adding && (
          <button type="button" className="ghost small" onClick={() => setAdding(true)}>
            <PlusIcon />
            Track a token
          </button>
        )}
      </div>
      {untrack.isError && <p className="error">{failureText(untrack.error)}</p>}
      {adding && (
        <form
          className="form panel"
          onSubmit={(event) => {
            event.preventDefault()
            if (!valid) return
            track.mutate(
              { tag: "Icrc", value: principal(typed) },
              {
                onSuccess: () => {
                  setAdding(false)
                  setLedger("")
                },
              },
            )
          }}
        >
          <label>
            ICRC-1 ledger canister id
            <input
              value={ledger}
              onChange={(event) => setLedger(event.target.value)}
              autoComplete="off"
              spellCheck={false}
            />
            {typed !== "" && !valid && <span className="error">That is not a canister id.</span>}
          </label>
          <div className="buttons">
            <button type="submit" className="primary" disabled={!valid || track.isPending}>
              {track.isPending ? "Tracking…" : "Track the token"}
            </button>
            <button
              type="button"
              className="ghost"
              disabled={track.isPending}
              onClick={() => setAdding(false)}
            >
              Cancel
            </button>
          </div>
          {track.isError && <p className="error">{failureText(track.error)}</p>}
        </form>
      )}
      <p className="field-label">Chains the EVM accounts show</p>
      <div className="chips">
        {chains.map((asset) => (
          <TokenChip
            key={chainKey(asset)}
            asset={asset}
            busy={untrack.isPending}
            onUntrack={owner ? () => untrack.mutate(asset) : undefined}
          />
        ))}
        {chains.length === 0 && <span className="quiet small">None</span>}
        {owner && untracked.length > 0 && !addingChain && (
          <button type="button" className="ghost small" onClick={() => setAddingChain(true)}>
            <PlusIcon />
            Track a chain
          </button>
        )}
      </div>
      {addingChain && (
        <form
          className="form panel"
          onSubmit={(event) => {
            event.preventDefault()
            const chain = untracked.find((candidate) => String(candidate.id) === chainId)
            if (chain === undefined) return
            trackChain.mutate(
              { tag: "Evm", value: chain.id },
              { onSuccess: () => setAddingChain(false) },
            )
          }}
        >
          <label>
            EVM chain
            <select
              value={chainId}
              onChange={(event) => setChainId(event.target.value)}
              disabled={trackChain.isPending}
            >
              {untracked.map((chain) => (
                <option key={String(chain.id)} value={String(chain.id)}>
                  {chain.name}
                </option>
              ))}
            </select>
          </label>
          <div className="buttons">
            <button type="submit" className="primary" disabled={trackChain.isPending}>
              {trackChain.isPending ? "Tracking…" : "Track the chain"}
            </button>
            <button
              type="button"
              className="ghost"
              disabled={trackChain.isPending}
              onClick={() => setAddingChain(false)}
            >
              Cancel
            </button>
          </div>
          {trackChain.isError && <p className="error">{failureText(trackChain.error)}</p>}
        </form>
      )}
    </div>
  )
}

function TokenChip({
  asset,
  busy,
  onUntrack,
}: {
  asset: Chain
  busy: boolean
  onUntrack: (() => void) | undefined
}) {
  const { token } = useToken(asset)
  const name =
    asset.tag === "Evm"
      ? chainText(asset)
      : (token?.symbol ?? (asset.tag === "Icrc" ? shortText(asset.value) : chainText(asset)))
  return (
    <span className="chip">
      {name}
      {onUntrack && (
        <button
          type="button"
          className="chip-remove"
          aria-label={`Stop tracking ${name}`}
          title={`Stop tracking ${name}`}
          disabled={busy}
          onClick={onUntrack}
        >
          <CrossIcon />
        </button>
      )}
    </span>
  )
}

type Kind = AccountKind["tag"]

/**
 * Adds another account of a kind, with an address of its own; it moves
 * nothing, so it needs no approval. Bitcoin accounts are of the wallet's
 * network; an EVM account has one address on every EVM chain.
 */
function AddAccount({
  wallet: id,
  network,
  onDone,
}: {
  wallet: Principal
  network: BitcoinNetwork
  onDone: () => void
}) {
  const { client, wallet } = useWallet(id)
  const add = useMutation(
    client.mutationOptions(wallet, "add_account", {
      invalidates: [
        [wallet, "accounts"],
        [wallet, "audit_log"],
      ],
    }),
  )
  const kinds: AccountKind[] = [
    { tag: "InternetComputer" },
    { tag: "Bitcoin", value: network },
    { tag: "Evm" },
  ]
  const [kind, setKind] = useState<Kind>("InternetComputer")
  const [label, setLabel] = useState("")
  const chosen = kinds.find((candidate) => candidate.tag === kind) ?? kinds[0]
  return (
    <form
      className="form panel"
      onSubmit={(event) => {
        event.preventDefault()
        if (chosen === undefined || !label.trim()) return
        add.mutate({ kind: chosen, label: label.trim() }, { onSuccess: onDone })
      }}
    >
      <h3>Add another account</h3>
      <fieldset className="choice">
        <legend>Kind</legend>
        <div className="segmented">
          {kinds.map((candidate) => (
            <label key={candidate.tag}>
              <input
                type="radio"
                name="kind"
                value={candidate.tag}
                checked={kind === candidate.tag}
                onChange={() => setKind(candidate.tag)}
              />
              <span>{kindText(candidate)}</span>
            </label>
          ))}
        </div>
      </fieldset>
      <label>
        Label
        <input
          value={label}
          onChange={(event) => setLabel(event.target.value)}
          placeholder="Savings"
          autoComplete="off"
        />
      </label>
      <div className="buttons">
        <button type="submit" className="primary" disabled={!label.trim() || add.isPending}>
          {add.isPending ? "Adding…" : "Add the account"}
        </button>
        <button type="button" className="ghost" disabled={add.isPending} onClick={onDone}>
          Cancel
        </button>
      </div>
      {add.isError && <p className="error">{failureText(add.error)}</p>}
    </form>
  )
}
