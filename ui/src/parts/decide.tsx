import type { Principal } from "@candid-core/schema"
import { useMutation } from "@tanstack/react-query"
import { useState } from "react"
import type { Pending } from "../generated/wallet.ts"
import { useWallet } from "../ic/canister.ts"
import { canRun, failureText, stateText } from "../logic/describe.ts"

/** What the last button did, which stays once its action leaves the list. */
export type Outcome = { id: bigint; pending: Pending } | { id: bigint; error: unknown }

/**
 * The calls behind the approve, reject and run buttons, wherever they are,
 * and what the last of them did.
 */
export function useDecide(id: Principal) {
  const { client, wallet } = useWallet(id)
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
  const busy = approve.isPending || reject.isPending || execute.isPending
  const run = (call: "approve" | "reject" | "execute", actionId: bigint) => {
    setOutcome(undefined)
    const mutation = call === "approve" ? approve : call === "reject" ? reject : execute
    mutation.mutate(actionId, {
      onSuccess: (result) => setOutcome({ id: actionId, pending: result }),
      onError: (error) => setOutcome({ id: actionId, error }),
    })
  }
  return { busy, outcome, run }
}

/** What the last button did to an action. */
export function OutcomeLine({ outcome, now }: { outcome: Outcome; now: bigint }) {
  return (
    <p className={"error" in outcome ? "error" : "notice"} role="status">
      Action {String(outcome.id)}:{" "}
      {"error" in outcome ? failureText(outcome.error) : stateText(outcome.pending, now)}
    </p>
  )
}

/**
 * Where an action stands, as the design system's status pill: a shape and a
 * word. Waiting for approvals is pending, approved but in its timelock is
 * scheduled, ready to run is confirmed, and a run is done.
 */
export function StateChip({ pending, now }: { pending: Pending; now: bigint }) {
  const { state } = pending
  const [look, text] =
    state.tag === "Waiting"
      ? ["pending", "Waiting"]
      : state.tag === "Ready"
        ? canRun(pending, now)
          ? ["confirmed", "Ready"]
          : ["scheduled", "Timelocked"]
        : state.tag === "Executed"
          ? ["done", "Done"]
          : state.tag === "Expired"
            ? ["pending", "Expired"]
            : ["failed", state.tag]
  return (
    <span className={`pill ${look}`}>
      <span className="pill-mark" aria-hidden="true" />
      {text}
    </span>
  )
}

/** An action's approvals, a dot each for as many as its quorum. */
export function ApprovalDots({ pending }: { pending: Pending }) {
  const count = pending.approvals.length
  return (
    <span className="dots" title={`${count} of ${pending.quorum} approvals`}>
      {Array.from({ length: pending.quorum }, (_, index) => (
        <span key={index} className={index < count ? "dot on" : "dot"} />
      ))}
      <span className="visually-hidden">
        {count} of {pending.quorum} approvals
      </span>
    </span>
  )
}
