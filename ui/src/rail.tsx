import type { Principal } from "@candid-core/schema"
import { useAuth } from "@ic-reactor/react"
import { skipToken, useQuery } from "@tanstack/react-query"
import { useEffect, useSyncExternalStore } from "react"
import { EventItem, NEWEST } from "./activity.tsx"
import { ApprovalDots, OutcomeLine, StateChip, useDecide } from "./approvals.tsx"
import { useWallet } from "./canister.ts"
import { CheckList } from "./checks.tsx"
import { CopyButton } from "./copy.tsx"
import {
  actionDetails,
  actionText,
  canRun,
  failureText,
  isJoinRequest,
  joinRequestText,
  sourceOf,
  waitsFor,
  whoText,
} from "./describe.ts"
import type { Account, Action, Member, Pending } from "./generated/wallet.ts"
import type { Check } from "./health.ts"
import { ArrowIcon, CheckIcon } from "./icons.tsx"
import { joinCode } from "./join-code.ts"
import { Reading, useMe } from "./member.tsx"
import { newestOf, readSeen, seenKey, subscribeSeen, unreadOf, writeSeen } from "./notifications.ts"
import { useNowNs } from "./now.ts"
import type { Page } from "./page.ts"
import { useHealth } from "./session.tsx"
import { SignIn } from "./sign-in.tsx"
import { agoText } from "./time.ts"
import { transferAsset, useToken } from "./tokens.ts"

/**
 * The rail beside the page on a wide screen: what needs you, then what
 * happened, on every page, so a request or a warning never waits behind a
 * click. Where the page itself lists those, the rail shows the wallet's
 * health instead. On a narrower screen the start page and the inbox show
 * the same panes in the page.
 */
export function Rail({ wallet: id, page }: { wallet: Principal; page: Exclude<Page, "missing"> }) {
  const auth = useAuth()
  const { me } = useMe(id)
  const member = me.isSuccess ? me.data : null
  if (member === null) {
    return (
      <div className="rail-inner">
        <HealthPane wallet={id} />
        {auth.status !== "signed-in" && (
          <section className="pane">
            <div className="pane-head">
              <h2>Members</h2>
            </div>
            <p className="quiet small">
              Sign in to see what waits for your approval and what happened in the wallet.
            </p>
            <SignIn />
          </section>
        )}
      </div>
    )
  }
  return (
    <div className="rail-inner">
      {page === "approvals" || page === "notifications" ? (
        <HealthPane wallet={id} />
      ) : (
        <NeedsYou wallet={id} me={member} />
      )}
      {page !== "notifications" && <Latest wallet={id} me={member} />}
    </div>
  )
}

/** Every check of the wallet's health, the worst first. */
export function HealthPane({ wallet: id }: { wallet: Principal }) {
  const health = useHealth(id)
  return (
    <section className="pane">
      <div className="pane-head">
        <h2>Wallet health</h2>
        <span className={`health-mark ${health.level}`} aria-hidden="true" />
      </div>
      {health.checks.length === 0 ? (
        <Reading>Checking the wallet…</Reading>
      ) : (
        <CheckList checks={health.checks} />
      )}
    </section>
  )
}

/**
 * What needs you: what is wrong or worth watching in the wallet, then each
 * open action you may approve or run, as a ticket with its buttons, so you
 * decide where you are. The rest of the open actions are a link away.
 */
export function NeedsYou({ wallet: id, me }: { wallet: Principal; me: Member }) {
  const { client, wallet } = useWallet(id)
  const health = useHealth(id)
  const pending = useQuery(client.queryOptions(wallet, "pending"))
  const accounts = useQuery(client.queryOptions(wallet, "accounts"))
  const members = useQuery(client.queryOptions(wallet, "members"))
  const { busy, outcome, run } = useDecide(id)
  const now = useNowNs()
  // The tickets below are the actions that wait for you.
  const alerts = health.checks.filter((check) => check.level !== "ok" && check.key !== "waiting")
  const mine = pending.data?.filter((action) => waitsFor(action, me, now)) ?? []
  const others = (pending.data?.length ?? 0) - mine.length
  const count = alerts.length + mine.length
  return (
    <section className="pane needs" aria-labelledby="needs-you">
      <div className="pane-head">
        <h2 id="needs-you">Needs you</h2>
        {pending.data !== undefined &&
          (count > 0 ? (
            <span className="badge">{count}</span>
          ) : (
            <span className="pane-calm">
              <CheckIcon />
              Clear
            </span>
          ))}
      </div>
      {outcome && <OutcomeLine outcome={outcome} now={now} />}
      {alerts.length > 0 && <Alerts wallet={id} alerts={alerts} />}
      {pending.isError ? (
        <p className="error">{failureText(pending.error)}</p>
      ) : pending.data === undefined ? (
        <Reading>Reading the open actions…</Reading>
      ) : mine.length > 0 ? (
        <ul className="tickets">
          {mine.map((action) => (
            <Ticket
              key={String(action.id)}
              pending={action}
              me={me}
              accounts={accounts.data}
              members={members.data}
              now={now}
              busy={busy}
              onApprove={() => run("approve", action.id)}
              onReject={() => run("reject", action.id)}
              onExecute={() => run("execute", action.id)}
            />
          ))}
        </ul>
      ) : (
        alerts.length === 0 && (
          <p className="calm">Nothing waits for you. New requests show up here first.</p>
        )
      )}
      {others > 0 && (
        <a className="pane-link" href="/approvals">
          {others} more open, waiting for others
          <ArrowIcon />
        </a>
      )}
    </section>
  )
}

/** Checks that need attention, each with what to do; low cycles with the top-up. */
function Alerts({ wallet: id, alerts }: { wallet: Principal; alerts: readonly Check[] }) {
  return (
    <ul className="alerts">
      {alerts.map((check) => (
        <li key={check.key ?? check.title} className={`alert ${check.level}`}>
          <span className={`health-mark ${check.level}`} aria-hidden="true" />
          <div>
            <p className="alert-title">
              {check.href ? <a href={check.href}>{check.title}</a> : check.title}
            </p>
            <p className="alert-text">{check.text}</p>
            {check.key === "cycles" && (
              <p className="command">
                <code>icp canister top-up --amount 1t {id}</code>
                <CopyButton
                  text={`icp canister top-up --amount 1t ${id}`}
                  label="Copy the command"
                />
              </p>
            )}
          </div>
        </li>
      ))}
    </ul>
  )
}

/** What kind of action a ticket is, in a word or two. */
function kindOf(pending: Pending): string {
  const names: Record<Action["tag"], string> = {
    Transfer: "Transfer",
    AddMember: "New member",
    RemoveMember: "Remove member",
    ChangeRole: "Role change",
    SetPolicy: "Policy change",
    Upgrade: "Upgrade",
    InstallExtension: "New extension",
    UninstallExtension: "Remove extension",
    HandOverControl: "Hand-over",
  }
  return isJoinRequest(pending) ? "Join request" : names[pending.action.tag]
}

/**
 * One action that waits for you: what it does, in full and with every
 * detail the approvals page shows, who proposed it and how many approvals
 * it has, and the buttons your role allows. A join request shows the code
 * the asking device must show.
 */
function Ticket({
  pending,
  me,
  accounts,
  members,
  now,
  busy,
  onApprove,
  onReject,
  onExecute,
}: {
  pending: Pending
  me: Member
  accounts: Account[] | undefined
  members: Member[] | undefined
  now: bigint
  busy: boolean
  onApprove: () => void
  onReject: () => void
  onExecute: () => void
}) {
  const { action, state } = pending
  const from = sourceOf(action, accounts)
  const { token } = useToken(transferAsset(action, from))
  const join = isJoinRequest(pending)
  // What the approvals page shows under an action, before its buttons: the
  // rules a policy change sets, what an extension is granted, a module's hash.
  const details = actionDetails(action)
  const ready = canRun(pending, now)
  const approved = pending.approvals.includes(me.id)
  const tone = join || action.tag !== "Transfer" ? "ticket watch" : "ticket"
  return (
    <li className={ready ? `${tone} ready` : tone}>
      <p className="ticket-head">
        <span className="ticket-kind">{kindOf(pending)}</span>
        <span className="ticket-id">#{String(pending.id)}</span>
        <StateChip pending={pending} now={now} />
      </p>
      <p className="ticket-text">
        {join ? joinRequestText(pending) : actionText(action, from, token)}.
      </p>
      {details.length > 0 && (
        <ul className="details">
          {details.map((line, index) => (
            <li key={index}>{line}</li>
          ))}
        </ul>
      )}
      {join && (
        <div className="ticket-code">
          <p>Approve only if the device that asks shows</p>
          <p className="code">{joinCode(pending.proposed_by)}</p>
        </div>
      )}
      <p className="ticket-meta">
        <ApprovalDots pending={pending} />
        <span>
          {whoText(pending.proposed_by, me, members)} · {agoText(pending.proposed_at_ns, now)}
        </span>
      </p>
      <div className="ticket-actions">
        {state.tag === "Waiting" && !approved && (
          <button type="button" className="primary small" disabled={busy} onClick={onApprove}>
            Approve
          </button>
        )}
        {ready && (
          <button type="button" className="primary small" disabled={busy} onClick={onExecute}>
            Run it
          </button>
        )}
        {me.role.tag === "Owner" && (
          <button type="button" className="danger small" disabled={busy} onClick={onReject}>
            Reject
          </button>
        )}
        <a className="ticket-more" href="/approvals">
          Details
        </a>
      </div>
    </li>
  )
}

/**
 * The newest entries of the audit log, with what is new since you last
 * looked marked, and the way to the inbox, which holds the rest.
 */
export function Latest({ wallet: id, me }: { wallet: Principal; me: Member }) {
  const { client, wallet } = useWallet(id)
  const log = useQuery(client.queryOptions(wallet, "audit_log", NEWEST))
  const members = useQuery(client.queryOptions(wallet, "members"))
  const accounts = useQuery(client.queryOptions(wallet, "accounts"))
  const key = seenKey(id, me.id)
  const seen = useSyncExternalStore(
    subscribeSeen,
    () => readSeen(key),
    () => undefined,
  )
  const fresh = new Set(unreadOf(log.data ?? [], seen, me.id).map((entry) => String(entry.seq)))
  const now = useNowNs()
  return (
    <section className="pane latest" aria-labelledby="latest">
      <div className="pane-head">
        <h2 id="latest">Latest</h2>
        {fresh.size > 0 && <span className="badge soft">{fresh.size} new</span>}
      </div>
      {log.isError ? (
        <p className="error">{failureText(log.error)}</p>
      ) : log.data === undefined ? (
        <Reading>Reading the audit log…</Reading>
      ) : log.data.length === 0 ? (
        <p className="calm">Nothing has happened yet.</p>
      ) : (
        <ol className="timeline compact">
          {log.data.map((entry) => (
            <EventItem
              key={String(entry.seq)}
              entry={entry}
              me={me}
              members={members.data}
              accounts={accounts.data}
              now={now}
              fresh={fresh.has(String(entry.seq))}
            />
          ))}
        </ol>
      )}
      <a className="pane-link" href="/notifications">
        Open the inbox
        <ArrowIcon />
      </a>
    </section>
  )
}

/**
 * The entries of the newest page `me` has not seen, for the dot in the bar.
 * It reads what the start page's activity reads, so there it costs no call.
 * On a first visit nothing is new: it remembers the newest entry as seen.
 */
export function useUnread(id: Principal, me: Member | undefined) {
  const { client, wallet } = useWallet(id)
  const log = useQuery(client.queryOptions(wallet, "audit_log", me ? NEWEST : skipToken))
  const key = seenKey(id, me?.id ?? "")
  const seen = useSyncExternalStore(
    subscribeSeen,
    () => readSeen(key),
    () => undefined,
  )
  useEffect(() => {
    if (me && log.data && seen === undefined) writeSeen(key, newestOf(log.data))
  }, [me, log.data, seen, key])
  const unread = me && log.data ? unreadOf(log.data, seen, me.id) : []
  // The newest page is all new: there may be more than it holds.
  return { count: unread.length, more: unread.length === NEWEST.limit }
}

/** A count beside a page in the spine, said in full to screen readers. */
function Count({ value, label }: { value: string; label: string }) {
  return (
    <span className="nav-count">
      <span aria-hidden="true">{value}</span>
      <span className="visually-hidden">, {label}</span>
    </span>
  )
}

/** How many open actions wait for you, beside Approvals. */
export function ApprovalsCount({ wallet: id }: { wallet: Principal }) {
  const { me } = useMe(id)
  const member = me.isSuccess ? me.data : null
  const { client, wallet } = useWallet(id)
  const pending = useQuery(client.queryOptions(wallet, "pending", member ? undefined : skipToken))
  const now = useNowNs()
  if (member === null || pending.data === undefined) return null
  const mine = pending.data.filter((action) => waitsFor(action, member, now)).length
  return mine === 0 ? null : (
    <Count value={String(mine)} label={`${mine} wait${mine === 1 ? "s" : ""} for you`} />
  )
}

/** How many notifications are new, beside Inbox. */
export function InboxCount({ wallet: id }: { wallet: Principal }) {
  const { me } = useMe(id)
  const unread = useUnread(id, me.isSuccess ? (me.data ?? undefined) : undefined)
  if (unread.count === 0) return null
  const value = `${unread.count}${unread.more ? "+" : ""}`
  return <Count value={value} label={`${value} new`} />
}
