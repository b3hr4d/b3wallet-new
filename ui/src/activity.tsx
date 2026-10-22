import type { Principal } from "@candid-core/schema"
import { useQuery } from "@tanstack/react-query"
import type { ReactNode } from "react"
import { useWallet } from "./canister.ts"
import { auditText, failureText, whoText } from "./describe.ts"
import type { Account, AuditEvent, Member } from "./generated/wallet.ts"
import {
  AlertIcon,
  CheckIcon,
  CrossIcon,
  DotIcon,
  KeyIcon,
  LayersIcon,
  PlusIcon,
  PulseIcon,
} from "./icons.tsx"
import { agoText, dateOf } from "./time.ts"

/** How many of the newest audit entries the start page shows. */
const SHOWN = 8

/** The newest entries of the wallet's audit log, newest first. */
export function Activity({
  wallet: id,
  me,
  members,
  accounts,
  now,
}: {
  wallet: Principal
  me: Member
  members: Member[] | undefined
  accounts: Account[] | undefined
  now: bigint
}) {
  const { client, wallet } = useWallet(id)
  const log = useQuery(client.queryOptions(wallet, "audit_log", { before_seq: null, limit: SHOWN }))
  return (
    <section className="card">
      <div className="card-head">
        <PulseIcon />
        <h2>Activity</h2>
      </div>
      {log.isError ? (
        <p className="error">{failureText(log.error)}</p>
      ) : log.data === undefined ? (
        <ul className="timeline">
          {[0, 1, 2].map((row) => (
            <li key={row} className="event">
              <span className="event-dot skeleton" />
              <span className="skeleton wide" />
            </li>
          ))}
        </ul>
      ) : log.data.length === 0 ? (
        <p className="quiet">Nothing has happened yet.</p>
      ) : (
        <ol className="timeline">
          {log.data.map((entry) => {
            const [tone, icon] = look(entry.event)
            return (
              <li key={String(entry.seq)} className="event">
                <span className={`event-dot ${tone}`}>{icon}</span>
                <p className="event-text">
                  <strong>{whoText(entry.by, me, members)}</strong>{" "}
                  {auditText(entry.event, accounts)}
                </p>
                <time
                  className="event-time"
                  dateTime={new Date(Number(entry.at_ns / 1_000_000n)).toISOString()}
                  title={dateOf(entry.at_ns)}
                >
                  {agoText(entry.at_ns, now)}
                </time>
              </li>
            )
          })}
        </ol>
      )}
    </section>
  )
}

/** The tone and icon of an event: done, refused, low, or just begun. */
function look(event: AuditEvent): [string, ReactNode] {
  switch (event.tag) {
    case "Approved":
    case "Executed":
    case "UpgradeInstalled":
      return ["good", <CheckIcon key="icon" />]
    case "Rejected":
    case "UpgradeFailed":
      return ["bad", <CrossIcon key="icon" />]
    case "CyclesLow":
      return ["warn", <AlertIcon key="icon" />]
    case "Claimed":
      return ["accent", <KeyIcon key="icon" />]
    case "UpgradeStaged":
      return ["accent", <LayersIcon key="icon" />]
    case "Proposed":
    case "JoinRequested":
    case "AccountAdded":
    case "AssetTracked":
      return ["accent", <PlusIcon key="icon" />]
    case "AssetUntracked":
    case "ExtensionCall":
      return ["", <DotIcon key="icon" />]
  }
}
