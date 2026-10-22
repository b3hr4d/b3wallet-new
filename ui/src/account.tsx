import type { Principal } from "@candid-core/schema"
import { useQueries, useQuery, type UseQueryResult } from "@tanstack/react-query"
import { useState } from "react"
import { KindAvatar, receivesText } from "./accounts.tsx"
import { StateChip } from "./approvals.tsx"
import { useWallet } from "./canister.ts"
import { CopyButton } from "./copy.tsx"
import { actionText, failureText } from "./describe.ts"
import type { Account, Balance, Chain, Member, Pending } from "./generated/wallet.ts"
import { numberParam } from "./host.ts"
import { ReceiveIcon, RefreshIcon, SendIcon } from "./icons.tsx"
import { AsMember, Reading } from "./member.tsx"
import { useNowNs } from "./now.ts"
import { agoText } from "./time.ts"
import {
  amountParts,
  assetsOf,
  chainKey,
  chainText,
  ICP_ONLY,
  kindText,
  transferAsset,
  useToken,
} from "./tokens.ts"

/**
 * An account's own page, at `/account?id=<id>`: what it holds of each asset,
 * where it receives, the transfers from it that are open, and the buttons
 * that send from it and receive to it.
 */
export function AccountPage({ wallet }: { wallet: Principal }) {
  return (
    <section className="column">
      <p className="back">
        <a href="/">Overview</a>
      </p>
      <AsMember wallet={wallet}>{(me) => <AccountView wallet={wallet} me={me} />}</AsMember>
    </section>
  )
}

function AccountView({ wallet: id, me }: { wallet: Principal; me: Member }) {
  const { client, wallet } = useWallet(id)
  const accounts = useQuery(client.queryOptions(wallet, "accounts"))
  const assets = useQuery(client.queryOptions(wallet, "assets"))
  const pending = useQuery(client.queryOptions(wallet, "pending"))
  const [accountId] = useState(() => numberParam(window.location.search, "id"))
  const account = accounts.data?.find((candidate) => candidate.id === accountId)
  const held = account ? assetsOf(account, assets.data ?? ICP_ONLY) : []
  // The same reads as the start page's, so a balance read there is not read again.
  const reads = useQueries({
    queries: held.map((asset) =>
      client.queryOptions(wallet, "balance", [account?.id ?? 0, asset], { update: "idempotent" }),
    ),
  })
  const now = useNowNs()

  if (accounts.isError) return <p className="card error">{failureText(accounts.error)}</p>
  if (accounts.data === undefined) {
    return (
      <div className="card">
        <Reading>Reading the account…</Reading>
      </div>
    )
  }
  if (account === undefined) {
    return (
      <div className="card empty">
        <h1>No such account</h1>
        <p className="quiet">
          This wallet has no account {accountId === undefined ? "here" : accountId}. Its start page
          lists every account.
        </p>
        <a className="button" href="/">
          Go to the start page
        </a>
      </div>
    )
  }
  const sends = me.role.tag === "Owner" || me.role.tag === "Proposer"
  const fetching = reads.some((read) => read.isFetching)
  const open = (pending.data ?? []).filter(
    (action) => action.action.tag === "Transfer" && action.action.value.from === account.id,
  )
  return (
    <div className="stack">
      <section className="card hero">
        <div className="hero-top">
          <KindAvatar kind={account.kind} />
          <div className="hero-name">
            <h1>{account.label}</h1>
            <p className="quiet small">
              {kindText(account.kind)} · account {account.index}
              {account.holder.tag === "Extension" ? ` of extension ${account.holder.value}` : ""}
            </p>
          </div>
        </div>
        <div className="holdings">
          {held.map((asset, index) => (
            <AssetBalance key={chainKey(asset)} asset={asset} read={reads[index]} now={now} />
          ))}
        </div>
        <div className="hero-actions">
          {sends && (
            <a className="button primary" href={`/send?from=${account.id}`}>
              <SendIcon />
              Send
            </a>
          )}
          <a className="button" href={`/receive?account=${account.id}`}>
            <ReceiveIcon />
            Receive
          </a>
          {reads.length > 0 && (
            <button
              type="button"
              className="ghost"
              disabled={fetching}
              onClick={() => reads.forEach((read) => void read.refetch())}
            >
              <RefreshIcon />
              {fetching ? "Reading…" : reads.length > 1 ? "Refresh the balances" : "Refresh"}
            </button>
          )}
        </div>
      </section>

      <section className="card">
        <h2>Addresses</h2>
        <div className="addresses">
          <p className="field-label">{receivesText(account.kind)}</p>
          <p className="address-box">
            <code>{account.address}</code>
            <CopyButton text={account.address} label="Copy the address" />
          </p>
          {account.icp_account_id !== null && (
            <>
              <p className="field-label">Its ICP account identifier, for exchanges</p>
              <p className="address-box">
                <code>{account.icp_account_id}</code>
                <CopyButton text={account.icp_account_id} label="Copy the account identifier" />
              </p>
            </>
          )}
        </div>
      </section>

      <section className="card">
        <div className="card-head">
          <h2>Open transfers</h2>
          {pending.data !== undefined && <span className="count">{open.length}</span>}
        </div>
        {pending.isError ? (
          <p className="error">{failureText(pending.error)}</p>
        ) : pending.data === undefined ? (
          <span className="skeleton wide" />
        ) : open.length === 0 ? (
          <p className="quiet">No transfer from this account is open.</p>
        ) : (
          <ul className="open-list">
            {open.map((action) => (
              <OpenTransfer key={String(action.id)} pending={action} account={account} now={now} />
            ))}
          </ul>
        )}
        <p className="quiet small">
          The wallet keeps no history per account yet; every action it runs is in the activity on
          the start page.
        </p>
      </section>
    </div>
  )
}

/** What the account holds of one asset, large, and when the wallet read it. */
function AssetBalance({
  asset,
  read,
  now,
}: {
  asset: Chain
  read: UseQueryResult<Balance> | undefined
  now: bigint
}) {
  const { token, error } = useToken(asset)
  const parts =
    token !== undefined && read?.data !== undefined
      ? amountParts(read.data.amount, token)
      : undefined
  return (
    <div className="holding">
      <p className="holding-label">
        <strong>{token?.symbol ?? chainText(asset)}</strong>
        {read?.data && <span>read {agoText(read.data.as_of_ns, now)}</span>}
      </p>
      {error !== undefined || read?.isError ? (
        <p className="holding-amount quiet" title={failureText(read?.error ?? error)}>
          Not read
        </p>
      ) : parts === undefined ? (
        <span className="skeleton big" />
      ) : (
        <p className="holding-amount">
          {parts.whole}
          <span className="amount-fraction">{parts.fraction}</span>
        </p>
      )}
    </div>
  )
}

/** One open transfer from the account: what it sends, and where it stands. */
function OpenTransfer({
  pending,
  account,
  now,
}: {
  pending: Pending
  account: Account
  now: bigint
}) {
  const { token } = useToken(transferAsset(pending.action, account))
  return (
    <li className="open-item">
      <p className="item-head">
        <strong>Action {String(pending.id)}</strong>
        <StateChip pending={pending} now={now} />
      </p>
      <p>{actionText(pending.action, account, token)}.</p>
      <a href="/approvals">Open it on the Approvals page</a>
    </li>
  )
}
