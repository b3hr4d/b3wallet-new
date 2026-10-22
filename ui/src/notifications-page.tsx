import type { Principal } from "@candid-core/schema"
import { useAuth } from "@ic-reactor/react"
import { skipToken, useQuery } from "@tanstack/react-query"
import { useEffect, useState, useSyncExternalStore } from "react"
import { EventItem, NEWEST } from "./activity.tsx"
import { useWallet } from "./canister.ts"
import { CheckList } from "./checks.tsx"
import { failureText } from "./describe.ts"
import type { Member } from "./generated/wallet.ts"
import { healthOf } from "./health.ts"
import { AsMember, Reading, useMe } from "./member.tsx"
import { newestOf, readSeen, seenKey, subscribeSeen, unreadOf, writeSeen } from "./notifications.ts"
import { useNowNs } from "./now.ts"

/** How many audit entries the page shows at a time. */
const PAGE = 20

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

/**
 * The notifications page: what needs you now, from the same checks as the
 * health dot, and what happened in the wallet, with what is new since you
 * last looked marked. Opening it marks everything as seen.
 */
export function NotificationsPage({ wallet }: { wallet: Principal }) {
  return (
    <section className="column">
      <p className="back">
        <a href="/">Overview</a>
      </p>
      <div className="page-head">
        <h1>Notifications</h1>
        <p className="quiet">What needs you, and what happened in this wallet.</p>
      </div>
      <AsMember wallet={wallet}>{(me) => <Inbox wallet={wallet} me={me} />}</AsMember>
    </section>
  )
}

function Inbox({ wallet: id, me }: { wallet: Principal; me: Member }) {
  const auth = useAuth()
  const { status } = useMe(id)
  const { client, wallet } = useWallet(id)
  const pending = useQuery(client.queryOptions(wallet, "pending"))
  const members = useQuery(client.queryOptions(wallet, "members"))
  const accounts = useQuery(client.queryOptions(wallet, "accounts"))
  const nowNs = useNowNs()
  const health = healthOf({
    status: status.data,
    statusFailure: status.isError ? failureText(status.error) : undefined,
    me,
    pending: pending.data,
    sessionEndsNs:
      auth.status === "signed-in" && auth.expiresAt !== undefined
        ? BigInt(auth.expiresAt) * 1_000_000n
        : undefined,
    nowNs,
  })
  const needs = health.checks.filter((check) => check.level !== "ok")

  const key = seenKey(id, me.id)
  // What was seen before this visit, so what is new stays marked while you read.
  const [seenBefore] = useState(() => readSeen(key))
  const [cursors, setCursors] = useState<bigint[]>([])
  const first = useQuery(
    client.queryOptions(wallet, "audit_log", { before_seq: null, limit: PAGE }),
  )
  useEffect(() => {
    if (first.data === undefined) return
    const newest = newestOf(first.data)
    const seen = readSeen(key)
    if (seen === undefined || newest > seen) writeSeen(key, newest)
  }, [first.data, key])
  const fresh = new Set(
    unreadOf(first.data ?? [], seenBefore, me.id).map((entry) => String(entry.seq)),
  )

  return (
    <div className="stack">
      <section className="card">
        <div className="card-head">
          <h2>Needs you</h2>
          {status.data !== undefined && <span className="count">{needs.length}</span>}
        </div>
        {status.data === undefined && !status.isError ? (
          <Reading>Checking the wallet…</Reading>
        ) : needs.length === 0 ? (
          <p className="all-good">
            <span className="health-mark ok" aria-hidden="true" />
            Nothing needs you.
          </p>
        ) : (
          <CheckList checks={needs} />
        )}
      </section>

      <section className="card">
        <div className="card-head">
          <h2>What happened</h2>
          {fresh.size > 0 && <span className="count fresh-count">{fresh.size} new</span>}
        </div>
        {first.isError ? (
          <p className="error">{failureText(first.error)}</p>
        ) : first.data === undefined ? (
          <Reading>Reading the audit log…</Reading>
        ) : first.data.length === 0 ? (
          <p className="quiet">Nothing has happened yet.</p>
        ) : (
          <>
            <ol className="timeline">
              {first.data.map((entry) => (
                <EventItem
                  key={String(entry.seq)}
                  entry={entry}
                  me={me}
                  members={members.data}
                  accounts={accounts.data}
                  now={nowNs}
                  fresh={fresh.has(String(entry.seq))}
                />
              ))}
              {cursors.map((before) => (
                <Older
                  key={String(before)}
                  wallet={id}
                  before={before}
                  me={me}
                  members={members.data}
                  now={nowNs}
                />
              ))}
            </ol>
            <More
              wallet={id}
              last={cursors.at(-1) ?? null}
              firstPage={first.data}
              onOlder={(before) => setCursors([...cursors, before])}
            />
          </>
        )}
      </section>
    </div>
  )
}

/** An older page of the audit log, entries before `before`. */
function Older({
  wallet: id,
  before,
  me,
  members,
  now,
}: {
  wallet: Principal
  before: bigint
  me: Member
  members: Member[] | undefined
  now: bigint
}) {
  const { client, wallet } = useWallet(id)
  const page = useQuery(
    client.queryOptions(wallet, "audit_log", { before_seq: before, limit: PAGE }),
  )
  if (page.isError) return <li className="error">{failureText(page.error)}</li>
  if (page.data === undefined) {
    return (
      <li className="event">
        <span className="event-dot skeleton" />
        <span className="skeleton wide" />
      </li>
    )
  }
  return (
    <>
      {page.data.map((entry) => (
        <EventItem
          key={String(entry.seq)}
          entry={entry}
          me={me}
          members={members}
          accounts={undefined}
          now={now}
        />
      ))}
    </>
  )
}

/**
 * The button that asks for the page before the last one shown, while the
 * last page shown was full; the audit log answers newest first.
 */
function More({
  wallet: id,
  last,
  firstPage,
  onOlder,
}: {
  wallet: Principal
  last: bigint | null
  firstPage: readonly { seq: bigint }[]
  onOlder: (before: bigint) => void
}) {
  const { client, wallet } = useWallet(id)
  const page = useQuery(
    client.queryOptions(
      wallet,
      "audit_log",
      last === null ? skipToken : { before_seq: last, limit: PAGE },
    ),
  )
  const shown = last === null ? firstPage : (page.data ?? [])
  const oldest = shown.at(-1)
  if (shown.length < PAGE || oldest === undefined) return null
  return (
    <div className="buttons more">
      <button type="button" className="small" onClick={() => onOlder(oldest.seq)}>
        Show older
      </button>
    </div>
  )
}
