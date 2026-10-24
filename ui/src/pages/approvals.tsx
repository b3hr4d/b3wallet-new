import type { Principal } from "@candid-core/schema"
import { useQuery } from "@tanstack/react-query"
import type { Account, Member, Pending } from "../generated/wallet.ts"
import { useWallet } from "../ic/canister.ts"
import { useToken } from "../ic/ledger.ts"
import {
  actionDetails,
  actionText,
  canRun,
  failureText,
  isJoinRequest,
  joinRequestText,
  roleText,
  sourceOf,
  stateText,
} from "../logic/describe.ts"
import { joinCode } from "../logic/join-code.ts"
import { dateOf } from "../logic/time.ts"
import { transferAsset } from "../logic/tokens.ts"
import { ApprovalDots, OutcomeLine, StateChip, useDecide } from "../parts/decide.tsx"
import { CheckIcon } from "../parts/icons.tsx"
import { AsMember, Reading } from "../parts/member.tsx"
import { useNowNs } from "../parts/now.ts"

/**
 * The approvals page: every open action, with what each member may do about
 * it. Owners propose and reject; owners and approvers approve and run; a
 * proposer only reads what waits.
 */
export function Approvals({ wallet }: { wallet: Principal }) {
  return (
    <section className="column">
      <div className="page-head">
        <p className="eyebrow">Open actions</p>
        <h1>Approvals</h1>
        <p className="quiet">
          Every open action, with what your role lets you do. An action runs once it has its
          approvals and its wait is over.
        </p>
      </div>
      <AsMember wallet={wallet}>{(me) => <OpenActions wallet={wallet} me={me} />}</AsMember>
    </section>
  )
}

function OpenActions({ wallet: id, me }: { wallet: Principal; me: Member }) {
  const { client, wallet } = useWallet(id)
  const pending = useQuery(client.queryOptions(wallet, "pending"))
  const accounts = useQuery(client.queryOptions(wallet, "accounts"))
  const members = useQuery(client.queryOptions(wallet, "members"))
  const { busy, outcome, run } = useDecide(id)
  const now = useNowNs()

  return (
    <>
      {outcome && <OutcomeLine outcome={outcome} now={now} />}
      {pending.isError ? (
        <p className="card error">{failureText(pending.error)}</p>
      ) : pending.data === undefined ? (
        <div className="card">
          <Reading>Reading the open actions…</Reading>
        </div>
      ) : pending.data.length === 0 ? (
        <div className="card empty">
          <p className="done-badge" aria-hidden="true">
            <CheckIcon />
          </p>
          <p>Nothing waits for approval.</p>
        </div>
      ) : (
        <ul className="list">
          {pending.data.map((action) => (
            <OpenAction
              key={String(action.id)}
              pending={action}
              accounts={accounts.data}
              members={members.data}
              me={me}
              now={now}
              busy={busy}
              onApprove={() => run("approve", action.id)}
              onReject={() => run("reject", action.id)}
              onExecute={() => run("execute", action.id)}
            />
          ))}
        </ul>
      )}
    </>
  )
}

function OpenAction({
  pending,
  accounts,
  members,
  me,
  now,
  busy,
  onApprove,
  onReject,
  onExecute,
}: {
  pending: Pending
  accounts: Account[] | undefined
  members: Member[] | undefined
  me: Member
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
  const details = actionDetails(action)
  const owner = me.role.tag === "Owner"
  const approves = owner || me.role.tag === "Approver"
  const approved = pending.approvals.includes(me.id)
  return (
    <li className="item">
      <p className="item-head">
        <strong>Action {String(pending.id)}</strong>
        <StateChip pending={pending} now={now} />
        <ApprovalDots pending={pending} />
      </p>
      <p>{join ? joinRequestText(pending) : actionText(action, from, token)}.</p>
      {details.length > 0 && (
        <ul className="details">
          {details.map((line, index) => (
            <li key={index}>{line}</li>
          ))}
        </ul>
      )}
      {join && (
        <>
          <p className="quiet">Approve it only if the device that asks shows this code:</p>
          <p className="code">{joinCode(pending.proposed_by)}</p>
        </>
      )}
      {!join && (
        <p className="quiet">
          Proposed by {proposerText(pending.proposed_by, me, members)} on{" "}
          {dateOf(pending.proposed_at_ns)}.
        </p>
      )}
      <p>
        {stateText(pending, now)}
        {approved && " You approved it."}
      </p>
      <p className="quiet">Open until {dateOf(pending.expires_at_ns)}.</p>
      <div className="buttons">
        {state.tag === "Waiting" && approves && !approved && (
          <button type="button" className="primary" disabled={busy} onClick={onApprove}>
            Approve
          </button>
        )}
        {approves && canRun(pending, now) && (
          <button type="button" className="primary" disabled={busy} onClick={onExecute}>
            Run it
          </button>
        )}
        {owner && (
          <button type="button" className="danger" disabled={busy} onClick={onReject}>
            Reject
          </button>
        )}
      </div>
    </li>
  )
}

/**
 * Who proposed an action: you, or a member by label and role, so a proposer
 * such as an AI agent stands out. A member who has left shows as a principal.
 */
export function proposerText(id: Principal, me: Member, members: Member[] | undefined): string {
  if (id === me.id) return "you"
  const member = members?.find((member) => member.id === id)
  return member ? `“${member.label}”, ${roleText(member.role)} (${id})` : id
}
