import type { Principal } from "@candid-core/schema"
import { skipToken, useMutation, useQuery } from "@tanstack/react-query"
import { useState } from "react"
import { useWallet } from "./canister.ts"
import { failureText } from "./describe.ts"
import {
  actor,
  type Actor,
  type GuardReport,
  type Member,
  type Pending,
} from "./generated/wallet.ts"
import { useClient } from "@ic-reactor/react"
import { dateOf } from "./time.ts"

/** The longest request the guard reads, in bytes of UTF-8. */
const MAX_REQUEST_BYTES = 280

/**
 * Requests (decision 0011): owners write here what they ask their agent to do,
 * in their own words. The agent reads the open ones and answers one with a
 * transfer, and the guard checks that transfer against these words, so no one
 * pastes anything and the agent cannot change what was asked.
 */
export function Requests({ wallet: id, me }: { wallet: Principal; me: Member }) {
  const { client, wallet } = useWallet(id)
  const requests = useQuery(client.queryOptions(wallet, "requests"))
  const ask = useMutation(
    client.mutationOptions(wallet, "ask", { invalidates: [[wallet, "requests"]] }),
  )
  const close = useMutation(
    client.mutationOptions(wallet, "close_request", { invalidates: [[wallet, "requests"]] }),
  )
  const [text, setText] = useState("")
  const owner = me.role.tag === "Owner"
  const bytes = new TextEncoder().encode(text.trim()).length
  return (
    <div className="card">
      <p className="item-head">
        <strong>Requests to your agent</strong>
      </p>
      <p className="quiet">
        What you ask your AI agent to do, in your own words. The agent answers a request with a
        transfer, and the guard checks that transfer against what you wrote here.
      </p>
      {owner && (
        <form
          className="form"
          onSubmit={(event) => {
            event.preventDefault()
            ask.mutate(text, { onSuccess: () => setText("") })
          }}
        >
          <label>
            Ask your agent
            <input
              value={text}
              onChange={(event) => setText(event.target.value)}
              placeholder="Send 1.5 ICP to rrkah-fqaaa-aaaaa-aaaaq-cai"
              autoComplete="off"
            />
            {bytes > MAX_REQUEST_BYTES && (
              <span className="error">At most {MAX_REQUEST_BYTES} bytes.</span>
            )}
          </label>
          <div className="buttons">
            <button
              type="submit"
              className="primary"
              disabled={bytes === 0 || bytes > MAX_REQUEST_BYTES || ask.isPending}
            >
              {ask.isPending ? "Writing…" : "Write the request"}
            </button>
          </div>
          {ask.isError && <p className="error">{failureText(ask.error)}</p>}
        </form>
      )}
      {requests.isError ? (
        <p className="error">{failureText(requests.error)}</p>
      ) : requests.data?.length === 0 ? (
        <p className="quiet">No open requests.</p>
      ) : (
        <ul className="details">
          {requests.data?.map((request) => (
            <li key={String(request.id)}>
              Request {String(request.id)}: “{request.text}”, {dateOf(request.asked_at_ns)}
              {request.answers.length > 0 &&
                `, answered by action ${request.answers.map(String).join(", ")}`}
              {owner && (
                <>
                  {" "}
                  <button
                    type="button"
                    disabled={close.isPending}
                    onClick={() => close.mutate(request.id)}
                  >
                    Close
                  </button>
                </>
              )}
            </li>
          ))}
        </ul>
      )}
      {close.isError && <p className="error">{failureText(close.error)}</p>}
    </div>
  )
}

const PRESSURE: Record<string, string> = {
  urgency: "urgency",
  authority: "a claim of authority",
  approved: "a claim it is already approved",
  instruction: "an instruction to you",
}

const REQUEST_FLAGS: Record<string, string> = {
  says_not_to: "your words say not to send",
  repeats: "your words ask for more than one transfer",
}

/**
 * What the guard says about a transfer: the request it answers and, field by
 * field, whether the transfer is what that request asked for. It reads the
 * wallet's own `guard_check`, a query one node answers for free; the check only
 * adds warnings, and one it could not make is never shown as OK.
 */
export function GuardLine({
  wallet: id,
  pending,
  me,
}: {
  wallet: Principal
  pending: Pending
  me: Member
}) {
  const client = useClient()
  // A plain query: the wallet refuses guard_check as a replicated call, which it would pay for.
  const wallet = client.canister<Actor>(actor, { id, certified: false })
  const asked = pending.request !== null
  const report = useQuery(
    client.queryOptions(wallet, "guard_check", asked ? pending.id : skipToken),
  )
  if (pending.action.tag !== "Transfer") return null
  if (!asked) {
    return pending.proposed_by === me.id ? null : (
      <p className="notice">
        This transfer answers no request of yours, so the guard could not check it.
      </p>
    )
  }
  if (report.isError) {
    return <p className="notice">The guard could not check it: {failureText(report.error)}</p>
  }
  if (report.data === undefined) return <p className="quiet">The guard is reading your request…</p>
  return <Verdict report={report.data} />
}

function Verdict({ report }: { report: GuardReport }) {
  const fields = (verdict: "Same" | "Differs" | "NotChecked") =>
    report.fields.filter((field) => field.verdict.tag === verdict).map((field) => field.field)
  const pressure = report.texts.filter((text) => text.flags.length > 0)
  const reasons = [
    ...fields("Differs").map((field) => `the ${field} differs from what you asked`),
    ...report.request_flags.map((flag) => REQUEST_FLAGS[flag] ?? flag),
  ]
  const [look, word] =
    report.overall.tag === "Differs"
      ? ["failed", "Flagged"]
      : report.overall.tag === "Matches"
        ? ["confirmed", "Matches"]
        : ["pending", "Not fully checked"]
  return (
    <div>
      <p>
        <span className={`pill ${look}`}>
          <span className="pill-mark" aria-hidden="true" />
          Guard: {word}
        </span>{" "}
        You asked: “{report.request}”
      </p>
      <ul className="details">
        {reasons.map((reason) => (
          <li key={reason}>{reason}</li>
        ))}
        {report.overall.tag === "Matches" && (
          <li>The amount, asset and recipient are what you asked.</li>
        )}
        {report.overall.tag !== "Differs" && fields("NotChecked").length > 0 && (
          <li>Not checked: {fields("NotChecked").join(", ")}.</li>
        )}
        {report.abstain && <li>The guard was not sure how to read your words.</li>}
        {pressure.map((text) => (
          <li key={text.name}>
            The {text.name === "label" ? "proposer's label" : text.name} “{text.text}” uses{" "}
            {text.matched
              .map((match) => `“${match.text}” (${PRESSURE[match.cue] ?? match.cue})`)
              .join(", ")}
            .
          </li>
        ))}
      </ul>
    </div>
  )
}
