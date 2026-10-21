import type { Principal } from "@candid-core/schema"
import { useMutation, useQuery } from "@tanstack/react-query"
import { useState } from "react"
import { useWallet } from "./canister.ts"
import { actionText, canRun, failureText, roleText, stateText } from "./describe.ts"
import type { Account, Pending } from "./generated/wallet.ts"
import { AsMember } from "./member.tsx"
import { useNowNs } from "./now.ts"
import { amountText, chainText, readAmount, useToken, type Token } from "./tokens.ts"

/** The send page: an owner proposes a transfer, and runs it once it may. */
export function Send({ wallet }: { wallet: Principal }) {
  return (
    <section>
      <h1>Send</h1>
      <AsMember wallet={wallet}>
        {(me) =>
          me.role.tag === "Owner" ? (
            <SendForm wallet={wallet} />
          ) : (
            <p>
              Only an owner proposes a transfer, and you are {roleText(me.role)} of this wallet. You
              approve and run transfers on the <a href="/approvals">Approvals page</a>.
            </p>
          )
        }
      </AsMember>
    </section>
  )
}

function SendForm({ wallet: id }: { wallet: Principal }) {
  const { client, wallet } = useWallet(id)
  const accounts = useQuery(client.queryOptions(wallet, "accounts"))
  const propose = useMutation(
    client.mutationOptions(wallet, "propose", { invalidates: [[wallet, "pending"]] }),
  )
  const [fromId, setFromId] = useState<number>()
  const [to, setTo] = useState("")
  const [typed, setTyped] = useState("")
  const usable = (accounts.data ?? []).filter((account) => account.chain.tag !== "Evm")
  const from = usable.find((account) => account.id === fromId) ?? usable[0]
  const { token, error: tokenError } = useToken(from?.chain)
  const amount = token && typed.trim() ? readAmount(typed, token.decimals) : undefined

  if (propose.isSuccess) {
    return (
      <Proposed
        wallet={id}
        proposed={propose.data}
        from={from}
        token={token}
        onAgain={() => {
          propose.reset()
          setTo("")
          setTyped("")
        }}
      />
    )
  }
  if (accounts.isError) return <p className="error">{failureText(accounts.error)}</p>
  if (accounts.data === undefined) return <p className="quiet">Reading the accounts…</p>
  if (from === undefined) {
    return (
      <p>
        This wallet has no account to send from. <a href="/">Add one on its start page.</a>
      </p>
    )
  }
  const bitcoin = from.chain.tag === "Bitcoin"
  return (
    <form
      className="form"
      onSubmit={(event) => {
        event.preventDefault()
        if (amount?.amount === undefined || !to.trim()) return
        propose.mutate({
          tag: "Transfer",
          value: { from: from.id, to: to.trim(), amount: amount.amount, fee: null, memo: null },
        })
      }}
    >
      <label>
        From
        <select
          value={from.id}
          onChange={(event) => {
            setFromId(Number(event.target.value))
            setTyped("")
          }}
        >
          {usable.map((account) => (
            <option key={account.id} value={account.id}>
              {account.label}, {chainText(account.chain)}
            </option>
          ))}
        </select>
      </label>
      <label>
        To
        <input
          value={to}
          onChange={(event) => setTo(event.target.value)}
          placeholder={bitcoin ? "A Bitcoin address" : "A principal or an ICRC-1 account"}
          autoComplete="off"
          spellCheck={false}
        />
      </label>
      <label>
        Amount{token ? ` in ${token.symbol}` : ""}
        <input
          value={typed}
          onChange={(event) => setTyped(event.target.value)}
          inputMode="decimal"
          placeholder="0.0"
          autoComplete="off"
        />
        {amount?.error !== undefined && <span className="error">{amount.error}</span>}
      </label>
      {tokenError !== undefined && <p className="error">{failureText(tokenError)}</p>}
      <p className="quiet">
        {bitcoin
          ? "The fee is the median rate of recent transactions, and counts toward the daily limit."
          : token?.fee != null
            ? `The ledger takes its fee of ${amountText(token.fee, token)} on top.`
            : "The ledger takes its fee on top."}
      </p>
      <div className="buttons">
        <button
          type="submit"
          className="primary"
          disabled={amount?.amount === undefined || !to.trim() || propose.isPending}
        >
          {propose.isPending ? "Proposing…" : "Propose the transfer"}
        </button>
      </div>
      {propose.isError && <p className="error">{failureText(propose.error)}</p>}
    </form>
  )
}

/**
 * A transfer just proposed. One the policy lets through at once is ready, and
 * runs on Send now; one that needs more approvals waits on the Approvals page.
 */
function Proposed({
  wallet: id,
  proposed,
  from,
  token,
  onAgain,
}: {
  wallet: Principal
  proposed: Pending
  from: Account | undefined
  token: Token | undefined
  onAgain: () => void
}) {
  const { client, wallet } = useWallet(id)
  // A transfer changes the open actions and a balance, and nothing else read here.
  const execute = useMutation(
    client.mutationOptions(wallet, "execute", {
      invalidates: [
        [wallet, "pending"],
        [wallet, "balance"],
      ],
    }),
  )
  const now = useNowNs()
  const current = execute.data ?? proposed
  return (
    <div className="block">
      <p>{actionText(current.action, from, token)}.</p>
      <p>
        Action {String(current.id)}. {stateText(current, now)}
      </p>
      {current.state.tag === "Waiting" && (
        <p>
          The other owners and approvers approve it on the <a href="/approvals">Approvals page</a>.
        </p>
      )}
      {execute.isError && (
        <p className="error">
          {failureText(execute.error)} <a href="/approvals">The Approvals page</a> shows where it
          stands.
        </p>
      )}
      <div className="buttons">
        {canRun(current, now) && (
          <button
            type="button"
            className="primary"
            disabled={execute.isPending}
            onClick={() => execute.mutate(current.id)}
          >
            {execute.isPending ? "Sending…" : "Send now"}
          </button>
        )}
        <button type="button" disabled={execute.isPending} onClick={onAgain}>
          Send something else
        </button>
      </div>
    </div>
  )
}
