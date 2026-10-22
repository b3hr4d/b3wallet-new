import type { Principal } from "@candid-core/schema"
import { formatUnits } from "@ic-reactor/core"
import { useQueries, useQuery, type UseQueryResult } from "@tanstack/react-query"
import type { ReactNode } from "react"
import { Accounts, Holdings, type BalanceRead } from "./accounts.tsx"
import { Activity } from "./activity.tsx"
import { useWallet } from "./canister.ts"
import { CopyButton } from "./copy.tsx"
import { actionText, failureText, isJoinRequest, stateText, waitsFor } from "./describe.ts"
import type { Member, Pending, Status, UpgradeState } from "./generated/wallet.ts"
import { Loader } from "./brand.tsx"
import { AlertIcon, ApproveIcon, ArrowIcon, BoltIcon, WalletMark } from "./icons.tsx"
import { JoinRequest } from "./join.tsx"
import { hueClass } from "./mark.ts"
import { Reading, useMe } from "./member.tsx"
import { useNowNs } from "./now.ts"
import { HomeOutline } from "./outline.tsx"
import { Members, Rules } from "./people.tsx"
import { Release } from "./release.tsx"
import { SignIn } from "./session.tsx"
import { dateOf } from "./time.ts"
import { assetsOf, ICP_ONLY, LOW_CYCLES } from "./tokens.ts"

/**
 * The wallet's start page: what it runs and who you are to it, then for a
 * member a dashboard of its holdings, open actions, accounts, activity,
 * members and policy, and for anyone else signed in a request to join.
 */
export function Home({ wallet: id }: { wallet: Principal }) {
  const { auth, status, claimed, me } = useMe(id)

  if (status.isError) {
    return (
      <div className="dashboard">
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
    <div className="dashboard">
      <Hero id={id} status={status.data}>
        <div className="facts-row">
          <span className="chip">Version {status.data.version}</span>
          <span className="chip">
            {formatUnits(status.data.cycles, 12, { maxFractionDigits: 2 })}T cycles
          </span>
        </div>
      </Hero>
      <Notices wallet={id} status={status.data} />
      {body}
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
  return (
    <div className="dashboard">
      <Hero id={id} status={status} me={me}>
        <Holdings accounts={accounts} balances={balances} />
      </Hero>
      <Notices wallet={id} status={status} pending={pending.data} now={now} />
      <div className="tiles">
        <ActionsTile pending={pending} me={me} now={now} />
        <CyclesTile cycles={status.cycles} />
        <Release wallet={id} me={me} version={status.version} />
      </div>
      <Accounts
        wallet={id}
        me={me}
        network={status.bitcoin_network}
        accounts={accounts}
        assets={assets}
        balances={balances}
        now={now}
      />
      <div className="split">
        <Activity wallet={id} me={me} members={members.data} accounts={accounts.data} now={now} />
        <div className="stack">
          <Members me={me} members={members} />
          <Rules wallet={id} members={members.data} />
        </div>
      </div>
    </div>
  )
}

/** The wallet's name, id and mark, with who you are to it and what it shows below. */
function Hero({
  id,
  status,
  me,
  children,
}: {
  id: Principal
  status?: Status
  me?: Member
  children?: ReactNode
}) {
  return (
    <section className={`card hero ${hueClass(id)}`}>
      <div className="hero-top">
        <WalletMark id={id} />
        <div className="hero-name">
          <h1>{me ? "Your wallet" : "Wallet"}</h1>
          <p className="hero-id">
            <code className="principal">{id}</code>
            <CopyButton text={id} label="Copy the wallet's id" />
          </p>
        </div>
        {me && (
          <p className="hero-chips">
            <span className="chip accent">
              {me.role.tag}
              {me.label ? ` · ${me.label}` : ""}
            </span>
          </p>
        )}
      </div>
      {status !== undefined && children}
    </section>
  )
}

/**
 * What a visitor should not miss: an upgrade under way or failed, a wallet
 * low on cycles, and for members every open action that is not a transfer,
 * since those change who controls the wallet or what it runs.
 */
function Notices({
  wallet: id,
  status,
  pending,
  now,
}: {
  wallet: Principal
  status: Status
  pending?: Pending[] | undefined
  now?: bigint
}) {
  const changes = (pending ?? []).filter(
    (action) => action.action.tag !== "Transfer" && !isJoinRequest(action),
  )
  const upgrade = upgradeNotice(status.upgrade)
  const low = status.cycles < LOW_CYCLES
  if (!upgrade && !low && changes.length === 0) return null
  return (
    <div className="notices">
      {upgrade}
      {low && (
        <Notice tone="warn" title="The wallet is low on cycles">
          <p>
            It has {formatUnits(status.cycles, 12, { maxFractionDigits: 2 })}T. An upgrade needs
            about 0.35T to spare. Anyone can top it up:
          </p>
          <p className="command">
            <code>icp canister top-up --amount 1t {id}</code>
            <CopyButton text={`icp canister top-up --amount 1t ${id}`} label="Copy the command" />
          </p>
        </Notice>
      )}
      {changes.map((action) => (
        <Notice
          key={String(action.id)}
          tone="warn"
          title={`${actionText(action.action, undefined, undefined)}.`}
          link={
            <a className="button small" href="/approvals">
              Review
            </a>
          }
        >
          <p>
            Action {String(action.id)}. {now === undefined ? "" : stateText(action, now)} If you did
            not expect it, an owner can reject it.
          </p>
        </Notice>
      ))}
    </div>
  )
}

function upgradeNotice(upgrade: UpgradeState): ReactNode {
  switch (upgrade.tag) {
    case "Idle":
      return null
    case "Staging":
      return (
        <Notice tone="info" title={`Staging version ${upgrade.value.version}`}>
          <p>
            {upgrade.value.chunks_uploaded} of {upgrade.value.chunks_total} chunks uploaded.
          </p>
        </Notice>
      )
    case "Installing":
      return (
        <Notice tone="info" title={`Installing version ${upgrade.value.version}`}>
          <p>The wallet answers again once the new version runs.</p>
        </Notice>
      )
    case "Failed":
      return (
        <Notice tone="bad" title={`The upgrade to version ${upgrade.value.version} failed`}>
          <p>
            On {dateOf(upgrade.value.at_ns)}: {upgrade.value.reason}
          </p>
        </Notice>
      )
  }
}

function Notice({
  tone,
  title,
  link,
  children,
}: {
  tone: "info" | "warn" | "bad"
  title: string
  link?: ReactNode
  children: ReactNode
}) {
  return (
    <div className={`notice-card ${tone}`}>
      <AlertIcon />
      <div className="notice-body">
        <p className="notice-title">{title}</p>
        {children}
      </div>
      {link}
    </div>
  )
}

/** How many actions are open, and how many wait for you. */
function ActionsTile({
  pending,
  me,
  now,
}: {
  pending: UseQueryResult<Pending[]>
  me: Member
  now: bigint
}) {
  const open = pending.data
  const mine = open?.filter((action) => waitsFor(action, me, now)).length ?? 0
  let note: string
  if (pending.isError) note = failureText(pending.error)
  else if (open === undefined) note = "Reading the open actions…"
  else if (open.length === 0) note = "Nothing waits for approval"
  else if (mine === 0) note = "None waits for you"
  else note = `${mine} wait${mine === 1 ? "s" : ""} for you`
  return (
    <a className="card tile tile-link" href="/approvals">
      <p className="tile-label">
        <ApproveIcon />
        Approvals
        <ArrowIcon />
      </p>
      <p className="tile-value">
        {open === undefined ? <span className="skeleton" /> : open.length}
        <span className="tile-unit">open</span>
      </p>
      <p
        className={
          mine > 0 ? "tile-note accent" : pending.isError ? "tile-note error" : "tile-note"
        }
      >
        {mine > 0 && <Loader waiting />}
        {note}
      </p>
    </a>
  )
}

/** Cycles drawn against the 5T a wallet is created with at least. */
const FULL_CYCLES = 5_000_000_000_000n

function CyclesTile({ cycles }: { cycles: bigint }) {
  const low = cycles < LOW_CYCLES
  const filled = Number(((cycles < FULL_CYCLES ? cycles : FULL_CYCLES) * 100n) / FULL_CYCLES)
  return (
    <section className="card tile">
      <p className="tile-label">
        <BoltIcon />
        Cycles
      </p>
      <p className="tile-value">
        {formatUnits(cycles, 12, { maxFractionDigits: 2 })}
        <span className="tile-unit">T</span>
      </p>
      <svg
        className={low ? "gauge low" : "gauge"}
        viewBox="0 0 100 6"
        preserveAspectRatio="none"
        aria-hidden="true"
      >
        <rect className="gauge-track" width="100" height="6" />
        <rect className="gauge-fill" width={Math.max(filled, 2)} height="6" />
        <rect className="gauge-mark" x="20" width="0.8" height="6" />
      </svg>
      <p className={low ? "tile-note warn" : "tile-note"}>
        {low ? "Low: top it up before an upgrade" : "They pay for its calls and storage"}
      </p>
    </section>
  )
}
