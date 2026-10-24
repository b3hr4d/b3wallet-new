import type { Principal } from "@candid-core/schema"
import { useMutation, useQuery } from "@tanstack/react-query"
import { useState } from "react"
import type { Account, Chain, Pending } from "../generated/wallet.ts"
import { useWallet } from "../ic/canister.ts"
import { useToken } from "../ic/ledger.ts"
import {
  actionText,
  canRun,
  failureText,
  roleText,
  sourceOf,
  stateText,
} from "../logic/describe.ts"
import { numberParam } from "../logic/host.ts"
import {
  amountText,
  assetsOf,
  chainKey,
  chainText,
  ICP_ONLY,
  kindText,
  readAmount,
  transferAsset,
} from "../logic/tokens.ts"
import { StateChip } from "../parts/decide.tsx"
import { AsMember, Reading } from "../parts/member.tsx"
import { useNowNs } from "../parts/now.ts"

/**
 * The send page: an owner proposes a transfer, and runs it once it may. A
 * proposer proposes one too, and it waits for an owner or an approver.
 */
export function Send({ wallet }: { wallet: Principal }) {
  return (
    <section className="column">
      <div className="page-head">
        <p className="eyebrow">Move funds</p>
        <h1>Send</h1>
        <p className="quiet">
          An owner or a proposer proposes a transfer. An owner&apos;s runs at once within the daily
          limit; above it, or from a proposer, it waits for the approvals the policy asks for.
        </p>
      </div>
      <AsMember wallet={wallet}>
        {(me) =>
          me.role.tag === "Owner" || me.role.tag === "Proposer" ? (
            <>
              {me.role.tag === "Proposer" && (
                <p className="notice">
                  You are a proposer: a transfer you propose waits for an owner or an approver to
                  approve and run it.
                </p>
              )}
              <SendForm wallet={wallet} />
            </>
          ) : (
            <p className="card">
              Only an owner or a proposer proposes a transfer, and you are {roleText(me.role)} of
              this wallet. You approve and run transfers on the{" "}
              <a href="/approvals">Approvals page</a>.
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
  const assets = useQuery(client.queryOptions(wallet, "assets"))
  const propose = useMutation(
    client.mutationOptions(wallet, "propose", { invalidates: [[wallet, "pending"]] }),
  )
  // `?from=<id>` opens the form on that account, as an account's page links it.
  const [fromId, setFromId] = useState(() => numberParam(window.location.search, "from"))
  const [assetKey, setAssetKey] = useState<string>()
  const [to, setTo] = useState("")
  const [typed, setTyped] = useState("")
  const usable = accounts.data ?? []
  const from = usable.find((account) => account.id === fromId) ?? usable[0]
  const choices = from ? assetsOf(from, assets.data ?? ICP_ONLY) : []
  const asset = choices.find((choice) => chainKey(choice) === assetKey) ?? choices[0]
  const { token, error: tokenError } = useToken(asset)
  const amount = token && typed.trim() ? readAmount(typed, token.decimals) : undefined

  if (propose.isSuccess) {
    return (
      <Proposed
        wallet={id}
        proposed={propose.data}
        accounts={accounts.data}
        onAgain={() => {
          propose.reset()
          setTo("")
          setTyped("")
        }}
      />
    )
  }
  if (assets.isError) return <p className="card error">{failureText(assets.error)}</p>
  if (accounts.isError) return <p className="card error">{failureText(accounts.error)}</p>
  if (accounts.data === undefined) {
    return (
      <div className="card">
        <Reading>Reading the accounts…</Reading>
      </div>
    )
  }
  if (from === undefined) {
    return (
      <p className="card">
        This wallet has no account to send from. <a href="/">Add one on its start page.</a>
      </p>
    )
  }
  const bitcoin = from.kind.tag === "Bitcoin"
  const evm = from.kind.tag === "Evm"
  return (
    <form
      className="card form"
      onSubmit={(event) => {
        event.preventDefault()
        if (asset === undefined || amount?.amount === undefined || !to.trim()) return
        propose.mutate({
          tag: "Transfer",
          value: {
            from: from.id,
            asset,
            to: to.trim(),
            amount: amount.amount,
            fee: null,
            memo: null,
          },
        })
      }}
    >
      <label>
        From
        <select
          value={from.id}
          onChange={(event) => {
            setFromId(Number(event.target.value))
            setAssetKey(undefined)
            setTyped("")
          }}
        >
          {usable.map((account) => (
            <option key={account.id} value={account.id}>
              {account.label}, {kindText(account.kind)}
            </option>
          ))}
        </select>
      </label>
      {evm && asset === undefined && (
        <p className="notice">
          The wallet tracks no EVM chain, so this account sends on none.{" "}
          <a href="/">Track one on the start page.</a>
        </p>
      )}
      {(choices.length > 1 || evm) && asset !== undefined && (
        <label>
          {evm ? "Chain" : "Token"}
          <select
            value={chainKey(asset)}
            onChange={(event) => {
              setAssetKey(event.target.value)
              setTyped("")
            }}
          >
            {choices.map((choice) => (
              <AssetOption key={chainKey(choice)} asset={choice} />
            ))}
          </select>
        </label>
      )}
      <label>
        To
        <input
          value={to}
          onChange={(event) => setTo(event.target.value)}
          placeholder={
            bitcoin
              ? "A Bitcoin address"
              : evm
                ? "An address: 0x and 40 hex digits"
                : "A principal or an ICRC-1 account"
          }
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
      <p className="quiet small">
        {bitcoin
          ? "The fee is the median rate of recent transactions, and counts toward the daily limit."
          : evm
            ? "The fee is what the chain charges when the transfer runs, at most twice its recent base fee and a tip for each unit of gas. That most counts toward the daily limit."
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
 * An asset to send, named by its token's symbol once its ledger says it, or
 * by its EVM chain.
 */
function AssetOption({ asset }: { asset: Chain }) {
  const { token } = useToken(asset)
  const name = asset.tag === "Evm" ? chainText(asset) : (token?.symbol ?? chainText(asset))
  return <option value={chainKey(asset)}>{name}</option>
}

/**
 * A transfer just proposed. One the policy lets through at once is ready, and
 * runs on Send now; one that needs more approvals waits on the Approvals page.
 * It names the account the wallet returned, since the form's selection may
 * have changed while the proposal was on its way.
 */
function Proposed({
  wallet: id,
  proposed,
  accounts,
  onAgain,
}: {
  wallet: Principal
  proposed: Pending
  accounts: Account[] | undefined
  onAgain: () => void
}) {
  const from = sourceOf(proposed.action, accounts)
  const { token } = useToken(transferAsset(proposed.action, from))
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
    <div className="card">
      <p className="item-head">
        <strong>Action {String(current.id)}</strong>
        <StateChip pending={current} now={now} />
      </p>
      <p>{actionText(current.action, from, token)}.</p>
      <p className="quiet">{stateText(current, now)}</p>
      {current.state.tag === "Waiting" && (
        <p>
          Owners and approvers approve it on the <a href="/approvals">Approvals page</a>.
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
