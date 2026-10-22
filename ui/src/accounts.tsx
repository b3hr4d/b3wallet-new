import { isPrincipal, principal, type Principal } from "@candid-core/schema"
import { useMutation, type UseQueryResult } from "@tanstack/react-query"
import { useState, type ReactNode } from "react"
import { useWallet } from "./canister.ts"
import { CopyButton } from "./copy.tsx"
import { failureText, shortText } from "./describe.ts"
import type { Account, Balance, Chain, Member } from "./generated/wallet.ts"
import { ChevronIcon, PlusIcon, ReceiveIcon, RefreshIcon } from "./icons.tsx"
import { agoText } from "./time.ts"
import { amountParts, chainKey, useToken, type Token } from "./tokens.ts"

type BalanceRead = UseQueryResult<Balance>

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
 * What the wallet holds, a total per token over its accounts, once every
 * balance of that token is read.
 */
export function Holdings({
  accounts,
  balances,
  owner,
}: {
  accounts: UseQueryResult<Account[]>
  balances: BalanceRead[]
  owner: boolean
}) {
  if (accounts.isError) return <p className="error">{failureText(accounts.error)}</p>
  if (accounts.data === undefined) {
    return (
      <div className="holdings">
        <span className="skeleton big" />
      </div>
    )
  }
  if (accounts.data.length === 0) {
    return (
      <p className="holdings-note">
        No accounts yet.{owner ? " Add one below to receive ICP, ICRC-1 tokens or Bitcoin." : ""}
      </p>
    )
  }
  const groups = new Map<string, { chain: Chain; reads: Array<BalanceRead | undefined> }>()
  accounts.data.forEach((account, index) => {
    const key = chainKey(account.chain)
    const group = groups.get(key) ?? { chain: account.chain, reads: [] }
    group.reads.push(balances[index])
    groups.set(key, group)
  })
  return (
    <div className="holdings">
      {[...groups].map(([key, group]) => (
        <Holding key={key} chain={group.chain} reads={group.reads} />
      ))}
    </div>
  )
}

/** One token's total, under its symbol and how many accounts hold it. */
function Holding({ chain, reads }: { chain: Chain; reads: Array<BalanceRead | undefined> }) {
  const { token, error } = useToken(chain)
  const amounts = reads.map((read) => read?.data?.amount)
  const failed = error !== undefined || reads.some((read) => read?.isError)
  const total =
    token === undefined || amounts.some((amount) => amount === undefined)
      ? undefined
      : amountParts(
          amounts.reduce<bigint>((sum, amount) => sum + (amount ?? 0n), 0n),
          token,
        )
  return (
    <div className="holding">
      <p className="holding-label">
        {token ? <strong>{token.symbol}</strong> : <span className="skeleton" />}
        <span>
          {reads.length} account{reads.length === 1 ? "" : "s"}
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
    </div>
  )
}

/**
 * The wallet's accounts with their balances, for every member, and the form
 * that adds one, for owners.
 */
export function Accounts({
  wallet: id,
  me,
  local,
  accounts,
  balances,
  now,
}: {
  wallet: Principal
  me: Member
  local: boolean
  accounts: UseQueryResult<Account[]>
  balances: BalanceRead[]
  now: bigint
}) {
  const owner = me.role.tag === "Owner"
  const [adding, setAdding] = useState(false)
  const reading = balances.some((balance) => balance.isFetching)
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
              onClick={() => balances.forEach((balance) => void balance.refetch())}
            >
              <RefreshIcon />
              {reading ? "Reading…" : "Refresh"}
            </button>
          )}
          {owner && (
            <button
              type="button"
              className="small"
              aria-expanded={adding}
              onClick={() => setAdding(!adding)}
            >
              <PlusIcon />
              Add account
            </button>
          )}
        </div>
      </div>
      {adding && <AddAccount wallet={id} local={local} onDone={() => setAdding(false)} />}
      {accounts.isError ? (
        <p className="error">{failureText(accounts.error)}</p>
      ) : accounts.data === undefined ? (
        <ul className="accounts">
          <li className="account-outline">
            <span className="avatar skeleton" />
            <span className="skeleton wide" />
          </li>
        </ul>
      ) : accounts.data.length === 0 ? (
        <p className="quiet">This wallet has no accounts yet.</p>
      ) : (
        <ul className="accounts">
          {accounts.data.map((account, index) => (
            <AccountItem key={account.id} account={account} balance={balances[index]} now={now} />
          ))}
        </ul>
      )}
    </section>
  )
}

/**
 * One account: its balance, which the wallet asks its ledger or the Bitcoin
 * canister for, and, opened, the whole address that receives funds.
 */
function AccountItem({
  account,
  balance,
  now,
}: {
  account: Account
  balance: BalanceRead | undefined
  now: bigint
}) {
  const token = useToken(account.chain)
  const failure = balance?.isError ? balance.error : token.error
  let shown: ReactNode
  if (failure !== undefined && failure !== null) {
    shown = <span className="error">Not read</span>
  } else if (balance?.data === undefined || token.token === undefined) {
    shown = <span className="skeleton" />
  } else {
    shown = <Amount amount={balance.data.amount} token={token.token} />
  }
  return (
    <li className="account">
      <details>
        <summary>
          <TokenAvatar chain={account.chain} token={token.token} />
          <span className="account-name">
            <strong>{account.label}</strong>
            <span>{chainName(account.chain, token.token)}</span>
          </span>
          <span className="account-balance">{shown}</span>
          <ChevronIcon />
        </summary>
        <div className="account-body">
          {failure !== undefined && failure !== null && (
            <p className="error">{failureText(failure)}</p>
          )}
          <p className="field-label">Receives at</p>
          <p className="address-box">
            <code>{account.address}</code>
            <CopyButton text={account.address} label="Copy the address" />
          </p>
          <div className="account-foot">
            <span className="quiet">
              {balance?.data ? `Balance read ${agoText(balance.data.as_of_ns, now)}` : ""}
            </span>
            <span className="buttons">
              <a className="button small" href={`/receive?account=${account.id}`}>
                <ReceiveIcon />
                Receive
              </a>
              {balance && (
                <button
                  type="button"
                  className="ghost small"
                  disabled={balance.isFetching}
                  onClick={() => void balance.refetch()}
                >
                  <RefreshIcon />
                  {balance.isFetching ? "Reading…" : "Refresh the balance"}
                </button>
              )}
            </span>
          </div>
        </div>
      </details>
    </li>
  )
}

/** A chain in a few words, for under an account's label. */
export function chainName(chain: Chain, token: Token | undefined): string {
  switch (chain.tag) {
    case "Icp":
      return "Internet Computer"
    case "Icrc":
      return `${token?.symbol ?? "ICRC-1"} · ledger ${shortText(chain.value)}`
    case "Bitcoin":
      return chain.value.tag === "Mainnet" ? "Bitcoin" : `Bitcoin ${chain.value.tag.toLowerCase()}`
    case "Evm":
      return `EVM chain ${chain.value}`
  }
}

/** A token by its symbol, in plain text: networks are named, never drawn. */
export function TokenAvatar({ chain, token }: { chain: Chain; token: Token | undefined }) {
  const text = chain.tag === "Bitcoin" ? "BTC" : (token?.symbol.slice(0, 4) ?? "")
  return (
    <span className="avatar token" aria-hidden="true">
      {text}
    </span>
  )
}

type Kind = "Icp" | "Icrc" | "Bitcoin"

const KINDS: readonly Kind[] = ["Icp", "Icrc", "Bitcoin"]

/**
 * The chain an owner picked, or undefined while an ICRC-1 ledger's id is not
 * a canister id. Bitcoin is mainnet's, or regtest on icp-cli's network.
 */
function chainOf(kind: Kind, ledger: string, local: boolean): Chain | undefined {
  switch (kind) {
    case "Icp":
      return { tag: "Icp" }
    case "Icrc":
      return isPrincipal(ledger) ? { tag: "Icrc", value: principal(ledger) } : undefined
    case "Bitcoin":
      return { tag: "Bitcoin", value: { tag: local ? "Regtest" : "Mainnet" } }
  }
}

/** Adds an account; it moves nothing, so it needs no approval. */
function AddAccount({
  wallet: id,
  local,
  onDone,
}: {
  wallet: Principal
  local: boolean
  onDone: () => void
}) {
  const { client, wallet } = useWallet(id)
  const add = useMutation(
    client.mutationOptions(wallet, "add_account", { invalidates: [[wallet, "accounts"]] }),
  )
  const [kind, setKind] = useState<Kind>("Icp")
  const [ledger, setLedger] = useState("")
  const [label, setLabel] = useState("")
  const chain = chainOf(kind, ledger.trim(), local)
  const names: Record<Kind, string> = {
    Icp: "ICP",
    Icrc: "ICRC-1 token",
    Bitcoin: local ? "Bitcoin regtest" : "Bitcoin",
  }
  return (
    <form
      className="form panel"
      onSubmit={(event) => {
        event.preventDefault()
        if (chain === undefined || !label.trim()) return
        add.mutate({ chain, label: label.trim() }, { onSuccess: onDone })
      }}
    >
      <h3>Add an account</h3>
      <fieldset className="choice">
        <legend>Chain</legend>
        <div className="segmented">
          {KINDS.map((k) => (
            <label key={k}>
              <input
                type="radio"
                name="chain"
                value={k}
                checked={kind === k}
                onChange={() => setKind(k)}
              />
              <span>{names[k]}</span>
            </label>
          ))}
        </div>
      </fieldset>
      {kind === "Icrc" && (
        <label>
          Ledger canister id
          <input
            value={ledger}
            onChange={(event) => setLedger(event.target.value)}
            autoComplete="off"
            spellCheck={false}
          />
          {ledger.trim() !== "" && chain === undefined && (
            <span className="error">That is not a canister id.</span>
          )}
        </label>
      )}
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
        <button
          type="submit"
          className="primary"
          disabled={chain === undefined || !label.trim() || add.isPending}
        >
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
