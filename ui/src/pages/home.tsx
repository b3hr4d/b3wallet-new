import type { Principal } from "@candid-core/schema"
import { formatUnits } from "@ic-reactor/core"
import { useQueries, useQuery, type UseQueryResult } from "@tanstack/react-query"
import type { ReactNode } from "react"
import type { Member, Pending, Status } from "../generated/wallet.ts"
import { useWallet } from "../ic/canister.ts"
import { failureText, waitsFor } from "../logic/describe.ts"
import { hueClass } from "../logic/mark.ts"
import { dateOf, durationText } from "../logic/time.ts"
import { assetsOf, ICP_ONLY, LOW_CYCLES } from "../logic/tokens.ts"
import { Accounts, Holdings, type BalanceRead } from "../parts/accounts.tsx"
import { Activity } from "../parts/activity.tsx"
import { Loader } from "../parts/brand.tsx"
import { CopyButton } from "../parts/copy.tsx"
import {
  ApproveIcon,
  BoltIcon,
  LayersIcon,
  PeopleIcon,
  ReceiveIcon,
  SendIcon,
  ShieldIcon,
  WalletMark,
} from "../parts/icons.tsx"
import { JoinRequest } from "../parts/join.tsx"
import { Reading, useMe } from "../parts/member.tsx"
import { useNowNs } from "../parts/now.ts"
import { HomeOutline } from "../parts/outline.tsx"
import { HealthPane, NeedsYou } from "../parts/rail.tsx"
import { ReleaseCheck } from "../parts/release.tsx"
import { SignIn } from "../parts/sign-in.tsx"

/**
 * The wallet's start page. For a member it is the whole wallet at a glance:
 * what it holds, with Send and Receive; its instruments (open approvals,
 * cycles, the policy, the members, the version); what needs you; and its
 * accounts. On a wide screen what needs you and what happened sit in the
 * rail beside it; narrower, they are on the page. Anyone else reads what the
 * wallet runs, and signs in or asks to join.
 */
export function Home({ wallet: id }: { wallet: Principal }) {
  const { auth, status, claimed, me } = useMe(id)

  if (status.isError) {
    return (
      <div className="overview">
        <Hero id={id} />
        <p className="card error">{failureText(status.error)}</p>
      </div>
    )
  }
  if (status.data === undefined) return <HomeOutline />
  if (claimed && auth.status === "signed-in" && me.isSuccess && me.data !== null) {
    return <Dashboard wallet={id} me={me.data} status={status.data} />
  }
  let body: ReactNode
  if (!claimed) {
    body = (
      <div className="card empty">
        <h2>No owner yet</h2>
        <p className="quiet">
          Whoever opens this wallet&apos;s claim link and signs in becomes its first owner.
        </p>
        {status.data.claim_expires_at_ns !== null && (
          <p className="chip">
            The claim link works until {dateOf(status.data.claim_expires_at_ns)}
          </p>
        )}
      </div>
    )
  } else if (auth.status !== "signed-in") {
    body = (
      <div className="card empty">
        <h2>Sign in to open the wallet</h2>
        <p className="quiet">
          Its members see the accounts, balances and approvals here once they sign in.
        </p>
        <SignIn />
      </div>
    )
  } else if (me.isError) {
    body = <p className="card error">{failureText(me.error)}</p>
  } else if (!me.isSuccess) {
    body = (
      <div className="card">
        <Reading>Reading who you are to the wallet…</Reading>
      </div>
    )
  } else {
    body = <JoinRequest wallet={id} />
  }
  return (
    <div className="overview">
      <Hero id={id}>
        <p className="chips">
          <span className="chip">Version {status.data.version}</span>
          <span className="chip">
            {formatUnits(status.data.cycles, 12, { maxFractionDigits: 2 })}T cycles
          </span>
        </p>
      </Hero>
      {body}
      <div className="rail-twin">
        <HealthPane wallet={id} />
      </div>
    </div>
  )
}

function Dashboard({ wallet: id, me, status }: { wallet: Principal; me: Member; status: Status }) {
  const { client, wallet } = useWallet(id)
  const accounts = useQuery(client.queryOptions(wallet, "accounts"))
  const assets = useQuery(client.queryOptions(wallet, "assets"))
  const pending = useQuery(client.queryOptions(wallet, "pending"))
  const members = useQuery(client.queryOptions(wallet, "members"))
  // One read per account and asset it shows, shared by the totals and the list.
  const holdings = (accounts.data ?? []).flatMap((account) =>
    assetsOf(account, assets.data ?? ICP_ONLY).map((asset) => ({ account, asset })),
  )
  const reads = useQueries({
    queries: holdings.map(({ account, asset }) =>
      client.queryOptions(wallet, "balance", [account.id, asset], { update: "idempotent" }),
    ),
  })
  const balances: BalanceRead[] = holdings.flatMap((holding, index) => {
    const read = reads[index]
    return read ? [{ ...holding, read }] : []
  })
  const now = useNowNs()
  const sends = me.role.tag === "Owner" || me.role.tag === "Proposer"
  return (
    <div className="overview">
      <Hero
        id={id}
        me={me}
        actions={
          <>
            {sends && (
              <a className="button primary" href="/send">
                <SendIcon />
                Send
              </a>
            )}
            <a className="button" href="/receive">
              <ReceiveIcon />
              Receive
            </a>
          </>
        }
      >
        <Holdings accounts={accounts} balances={balances} />
      </Hero>
      <div className="rail-twin">
        <NeedsYou wallet={id} me={me} />
      </div>
      <Instruments wallet={id} me={me} status={status} pending={pending} members={members} />
      <Accounts
        wallet={id}
        me={me}
        network={status.bitcoin_network}
        accounts={accounts}
        assets={assets}
        balances={balances}
      />
      <div className="rail-twin">
        <Activity wallet={id} me={me} members={members.data} accounts={accounts.data} now={now} />
      </div>
    </div>
  )
}

/** The wallet's mark, name and id, who you are to it, its actions, and what it holds. */
function Hero({
  id,
  me,
  actions,
  children,
}: {
  id: Principal
  me?: Member
  actions?: ReactNode
  children?: ReactNode
}) {
  return (
    <section className={`hero ${hueClass(id)}`}>
      <div className="hero-head">
        <WalletMark id={id} />
        <div className="hero-name">
          <p className="eyebrow">
            {me ? `${me.role.tag}${me.label ? ` · ${me.label}` : ""}` : "B3Wallet"}
          </p>
          <h1>{me ? "Your wallet" : "Wallet"}</h1>
          <p className="hero-id">
            <code className="principal">{id}</code>
            <CopyButton text={id} label="Copy the wallet's id" />
          </p>
        </div>
        {actions && <div className="hero-actions">{actions}</div>}
      </div>
      {children}
    </section>
  )
}

/** Cycles drawn against the 5T a wallet is created with at least. */
const FULL_CYCLES = 5_000_000_000_000n

/** The cells of the cycles gauge. */
const CELLS = 24

/**
 * The wallet's instruments, a cell each: the open approvals and how many
 * wait for you, the cycles, what a transfer needs, the members and the
 * version. Each that has a page of its own opens it.
 */
function Instruments({
  wallet: id,
  me,
  status,
  pending,
  members,
}: {
  wallet: Principal
  me: Member
  status: Status
  pending: UseQueryResult<Pending[]>
  members: UseQueryResult<Member[]>
}) {
  const { client, wallet } = useWallet(id)
  const policy = useQuery(client.queryOptions(wallet, "policy"))
  const now = useNowNs()
  const open = pending.data
  const mine = open?.filter((action) => waitsFor(action, me, now)).length ?? 0
  const low = status.cycles < LOW_CYCLES
  const filled = Number(
    ((status.cycles < FULL_CYCLES ? status.cycles : FULL_CYCLES) * BigInt(CELLS)) / FULL_CYCLES,
  )
  const floor = Number((LOW_CYCLES * BigInt(CELLS)) / FULL_CYCLES)
  const roster = members.data ?? []
  const approvers = roster.filter(
    (member) => member.role.tag === "Owner" || member.role.tag === "Approver",
  ).length
  const owners = roster.filter((member) => member.role.tag === "Owner").length
  const transfer = policy.data?.rules.find((rule) => rule.kind.tag === "Transfer")
  return (
    <section className="instruments" aria-label="At a glance">
      <a className={mine > 0 ? "instrument hot" : "instrument"} href="/approvals">
        <p className="instrument-label">
          <ApproveIcon />
          Approvals
        </p>
        <p className="instrument-value">
          {open === undefined ? <span className="skeleton" /> : open.length}
          <span className="unit">open</span>
        </p>
        <p className="instrument-note">
          {pending.isError ? (
            failureText(pending.error)
          ) : mine > 0 ? (
            <>
              <Loader waiting />
              {mine} wait{mine === 1 ? "s" : ""} for you
            </>
          ) : open?.length === 0 ? (
            "Nothing open"
          ) : (
            "None waits for you"
          )}
        </p>
      </a>
      <div className={low ? "instrument warn" : "instrument"}>
        <p className="instrument-label">
          <BoltIcon />
          Cycles
        </p>
        <p className="instrument-value">
          {formatUnits(status.cycles, 12, { maxFractionDigits: 2 })}
          <span className="unit">T</span>
        </p>
        <svg
          className="gauge"
          viewBox={`0 0 ${CELLS * 4} 8`}
          preserveAspectRatio="none"
          aria-hidden="true"
        >
          {Array.from({ length: CELLS }, (_, cell) => (
            <rect
              key={cell}
              x={cell * 4}
              width="3"
              height="8"
              className={cell < Math.max(filled, 1) ? "on" : cell < floor ? "floor" : undefined}
            />
          ))}
        </svg>
        <p className="instrument-note">
          {low ? "Low: top it up before an upgrade" : "They pay for its calls and storage"}
        </p>
      </div>
      <a className="instrument" href="/settings#security">
        <p className="instrument-label">
          <ShieldIcon />A transfer needs
        </p>
        <p className="instrument-value">
          {transfer === undefined ? <span className="skeleton" /> : transfer.quorum}
          <span className="unit">
            {members.data === undefined ? "" : `of ${approvers}`} approval
            {transfer?.quorum === 1 ? "" : "s"}
          </span>
        </p>
        <p className="instrument-note">
          {transfer === undefined
            ? "Reading the policy…"
            : transfer.timelock_ns === 0n
              ? transfer.daily_limits.length > 0
                ? "No wait, with daily limits"
                : "No wait once approved"
              : `Then waits ${durationText(transfer.timelock_ns)}`}
        </p>
      </a>
      <a className="instrument" href="/members">
        <p className="instrument-label">
          <PeopleIcon />
          Members
        </p>
        <p className="instrument-value">
          {members.data === undefined ? <span className="skeleton" /> : roster.length}
        </p>
        <p className="instrument-note">
          {members.data === undefined
            ? "Reading the members…"
            : `${owners} owner${owners === 1 ? "" : "s"}, ${approvers} approve`}
        </p>
      </a>
      <div className="instrument">
        <p className="instrument-label">
          <LayersIcon />
          Version
        </p>
        <p className="instrument-value">{status.version}</p>
        <ReleaseCheck wallet={id} me={me} />
      </div>
    </section>
  )
}
