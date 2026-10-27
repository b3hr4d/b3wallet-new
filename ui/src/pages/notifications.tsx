import type { Principal } from "@candid-core/schema"
import { skipToken, useQuery } from "@tanstack/react-query"
import { useEffect, useState } from "react"
import type { Member } from "../generated/wallet.ts"
import { useWallet } from "../ic/canister.ts"
import { failureText } from "../logic/describe.ts"
import { newestOf, seenKey, unreadOf } from "../logic/notifications.ts"
import { EventItem } from "../parts/activity.tsx"
import { AsMember, Reading } from "../parts/member.tsx"
import { useNowNs } from "../parts/now.ts"
import { NeedsYou } from "../parts/rail.tsx"
import { readSeen, writeSeen } from "../parts/seen.ts"

/** How many audit entries the page shows at a time. */
const PAGE = 20

/**
 * The inbox: what needs you, with the same tickets as the rail, and what
 * happened in the wallet, with what is new since you last looked marked.
 * Opening it marks everything as seen. On a phone it is where requests and
 * notifications live.
 */
export function NotificationsPage({ wallet }: { wallet: Principal }) {
  return (
    <section className="column">
      <div className="page-head">
        <p className="eyebrow">Requests and notifications</p>
        <h1>Inbox</h1>
        <p className="quiet">What needs you, and everything that happened in this wallet.</p>
      </div>
      <AsMember wallet={wallet}>{(me) => <Inbox wallet={wallet} me={me} />}</AsMember>
    </section>
  )
}

function Inbox({ wallet: id, me }: { wallet: Principal; me: Member }) {
  const { client, wallet } = useWallet(id)
  const members = useQuery(client.queryOptions(wallet, "members"))
  const accounts = useQuery(client.queryOptions(wallet, "accounts"))
  const nowNs = useNowNs()

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
      <NeedsYou wallet={id} me={me} />

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
