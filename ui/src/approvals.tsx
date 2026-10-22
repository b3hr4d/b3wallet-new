import type { Principal } from "@candid-core/schema"
import { useMutation, useQuery } from "@tanstack/react-query"
import { useState } from "react"
import { useWallet } from "./canister.ts"
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
} from "./describe.ts"
import type { Account, Member, Pending } from "./generated/wallet.ts"
import { joinCode } from "./join-code.ts"
import { AsMember } from "./member.tsx"
import { useNowNs } from "./now.ts"
import { dateOf } from "./time.ts"
import { useToken } from "./tokens.ts"

/**
 * The approvals page: every open action, with what each member may do about
 * it. Owners propose and reject; owners and approvers approve and run; a
 * proposer only reads what waits.
 */
export function Approvals({ wallet }: { wallet: Principal }) {
  return (
    <section>
      <h1>Approvals</h1>
      <AsMember wallet={wallet}>{(me) => <OpenActions wallet={wallet} me={me} />}</AsMember>
    </section>
  )
}

/** What the last button did, which stays once its action leaves the list. */
type Outcome = { id: bigint; pending: Pending } | { id: bigint; error: unknown }

function OpenActions({ wallet: id, me }: { wallet: Principal; me: Member }) {
  const { client, wallet } = useWallet(id)
  const pending = useQuery(client.queryOptions(wallet, "pending"))
  const accounts = useQuery(client.queryOptions(wallet, "accounts"))
  const members = useQuery(client.queryOptions(wallet, "members"))
  const approve = useMutation(
    client.mutationOptions(wallet, "approve", { invalidates: [[wallet, "pending"]] }),
  )
  const reject = useMutation(
    client.mutationOptions(wallet, "reject", { invalidates: [[wallet, "pending"]] }),
  )
  // Running an action can change anything the pages read: members, balances,
  // the upgrade.
  const execute = useMutation(client.mutationOptions(wallet, "execute"))
  const [outcome, setOutcome] = useState<Outcome>()
  const now = useNowNs()
  const busy = approve.isPending || reject.isPending || execute.isPending

  const run = (call: typeof approve.mutate, actionId: bigint) => {
    setOutcome(undefined)
    call(actionId, {
      onSuccess: (result) => setOutcome({ id: actionId, pending: result }),
      onError: (error) => setOutcome({ id: actionId, error }),
    })
  }

  return (
    <>
      {outcome && (
        <p className={"error" in outcome ? "error" : "notice"}>
          Action {String(outcome.id)}:{" "}
          {"error" in outcome ? failureText(outcome.error) : stateText(outcome.pending, now)}
        </p>
      )}
      {pending.isError ? (
        <p className="error">{failureText(pending.error)}</p>
      ) : pending.data === undefined ? (
        <p className="quiet">Reading the open actions…</p>
      ) : pending.data.length === 0 ? (
        <p>Nothing waits for approval.</p>
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
              onApprove={() => run(approve.mutate, action.id)}
              onReject={() => run(reject.mutate, action.id)}
              onExecute={() => run(execute.mutate, action.id)}
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
  const { token } = useToken(from?.chain)
  const join = isJoinRequest(pending)
  const details = actionDetails(action)
  const owner = me.role.tag === "Owner"
  const approves = owner || me.role.tag === "Approver"
  const approved = pending.approvals.includes(me.id)
  return (
    <li className="item">
      <p className="item-head">
        <strong>Action {String(pending.id)}</strong>
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
          <button type="button" disabled={busy} onClick={onReject}>
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
function proposerText(id: Principal, me: Member, members: Member[] | undefined): string {
  if (id === me.id) return "you"
  const member = members?.find((member) => member.id === id)
  return member ? `“${member.label}”, ${roleText(member.role)} (${id})` : id
}
