import { isPrincipal, principal, type Principal } from "@candid-core/schema"
import { useMutation, useQuery } from "@tanstack/react-query"
import { useState } from "react"
import { useWallet } from "./canister.ts"
import { CopyButton } from "./copy.tsx"
import { failureText } from "./describe.ts"
import type { Account, Chain, Member } from "./generated/wallet.ts"
import { amountText, chainText, useToken } from "./tokens.ts"

/**
 * The wallet's accounts with their balances, for every member, and the form
 * that adds one, for owners.
 */
export function Accounts({
  wallet: id,
  me,
  local,
}: {
  wallet: Principal
  me: Member
  local: boolean
}) {
  const { client, wallet } = useWallet(id)
  const accounts = useQuery(client.queryOptions(wallet, "accounts"))
  const owner = me.role.tag === "Owner"
  return (
    <section className="block">
      <h2>Accounts</h2>
      {accounts.isError ? (
        <p className="error">{failureText(accounts.error)}</p>
      ) : accounts.data === undefined ? (
        <p className="quiet">Reading the accounts…</p>
      ) : accounts.data.length === 0 ? (
        <p>This wallet has no accounts yet.</p>
      ) : (
        <ul className="list">
          {accounts.data.map((account) => (
            <AccountItem key={account.id} wallet={id} account={account} />
          ))}
        </ul>
      )}
      {owner && <AddAccount wallet={id} local={local} />}
    </section>
  )
}

/**
 * One account: its balance, which the wallet asks its ledger or the Bitcoin
 * canister for, and the address that receives funds.
 */
function AccountItem({ wallet: id, account }: { wallet: Principal; account: Account }) {
  const { client, wallet } = useWallet(id)
  const token = useToken(account.chain)
  // An update that changes nothing, read once per page and on Refresh.
  const balance = useQuery(
    client.queryOptions(wallet, "balance", account.id, { update: "idempotent" }),
  )
  let shown
  if (balance.isError) {
    shown = <span className="error">{failureText(balance.error)}</span>
  } else if (token.error !== undefined) {
    shown = <span className="error">{failureText(token.error)}</span>
  } else if (balance.data === undefined || token.token === undefined) {
    shown = <span className="quiet">Reading the balance…</span>
  } else {
    shown = amountText(balance.data.amount, token.token)
  }
  return (
    <li className="item">
      <p className="item-head">
        <strong>{account.label}</strong> <span className="quiet">{chainText(account.chain)}</span>
      </p>
      <p className="amount">{shown}</p>
      <p className="quiet">Receives at</p>
      <p className="address">
        <code className="principal">{account.address}</code> <CopyButton text={account.address} />
      </p>
      <button
        type="button"
        className="small"
        disabled={balance.isFetching}
        onClick={() => void balance.refetch()}
      >
        {balance.isFetching ? "Reading…" : "Refresh the balance"}
      </button>
    </li>
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
function AddAccount({ wallet: id, local }: { wallet: Principal; local: boolean }) {
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
    Icrc: "An ICRC-1 token",
    Bitcoin: local ? "Bitcoin regtest" : "Bitcoin",
  }
  return (
    <form
      className="form"
      onSubmit={(event) => {
        event.preventDefault()
        if (chain === undefined || !label.trim()) return
        add.mutate(
          { chain, label: label.trim() },
          {
            onSuccess: () => {
              setLabel("")
              setLedger("")
            },
          },
        )
      }}
    >
      <h3>Add an account</h3>
      <label>
        Chain
        <select
          value={kind}
          onChange={(event) => setKind(KINDS.find((k) => k === event.target.value) ?? "Icp")}
        >
          {KINDS.map((k) => (
            <option key={k} value={k}>
              {names[k]}
            </option>
          ))}
        </select>
      </label>
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
      </div>
      {add.isError && <p className="error">{failureText(add.error)}</p>}
    </form>
  )
}
