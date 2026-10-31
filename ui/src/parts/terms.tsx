import type { Principal } from "@candid-core/schema"
import { skipToken, useMutation, useQuery } from "@tanstack/react-query"
import type { Account, Member, Pending, Request } from "../generated/wallet.ts"
import { useWallet } from "../ic/canister.ts"
import { failureText } from "../logic/describe.ts"
import {
  checkTransfer,
  confirmedTerms,
  endOf,
  readDraft,
  type DraftRead,
  type LedgerToken,
} from "../logic/terms.ts"
import { dateOf } from "../logic/time.ts"

/** The most drafts of terms a request takes; after that an owner writes a new request. */
const MAX_DRAFTS = 3

/**
 * The terms of an owner's request (decision 0014): what the agent read in
 * it, the token, who is paid, the most and the latest end, in words, and
 * an owner's yes or no to them. Until an owner confirms them, the wallet
 * refuses a transfer for the request, and the Approvals page checks the one
 * the agent then proposes against them.
 */
export function RequestTermsView({
  wallet: id,
  request,
  me,
  tokens,
  accounts,
  now,
}: {
  wallet: Principal
  request: Request
  me: Member
  tokens: LedgerToken[] | undefined
  accounts: Account[] | undefined
  now: bigint
}) {
  const { client, wallet } = useWallet(id)
  const invalidates = [
    [wallet, "requests"],
    [wallet, "request"],
  ] as const
  const confirm = useMutation(client.mutationOptions(wallet, "confirm_terms", { invalidates }))
  const decline = useMutation(client.mutationOptions(wallet, "decline_terms", { invalidates }))
  const terms = request.terms
  if (terms === null) {
    return (
      <p className="quiet">
        No terms yet. Once your agent writes the terms it reads here, it can propose a transfer for
        this request only after an owner confirms them.
      </p>
    )
  }
  const owner = me.role.tag === "Owner"
  const fixed = request.answers.length > 0
  const busy = confirm.isPending || decline.isPending
  const sayNo = (label: string) =>
    owner &&
    !fixed && (
      <button
        type="button"
        disabled={busy}
        onClick={() => decline.mutate([request.id, terms.version])}
      >
        {label}
      </button>
    )
  const errors = (
    <>
      {confirm.isError && <p className="error">{failureText(confirm.error)}</p>}
      {decline.isError && <p className="error">{failureText(decline.error)}</p>}
    </>
  )
  const who = (by: Principal) => (by === me.id ? "You" : "An owner")

  if (terms.confirmed !== null) {
    return (
      <div>
        <p>
          <span className="pill confirmed">
            <span className="pill-mark" aria-hidden="true" />
            Terms confirmed
          </span>{" "}
          “{terms.confirmed.text}”
        </p>
        <p className="quiet">
          {who(terms.confirmed.by)} confirmed draft {terms.version} on{" "}
          {dateOf(terms.confirmed.at_ns)}. The Approvals page checks your agent's transfer against
          these terms.
        </p>
        <div className="buttons">{sayNo("Withdraw the terms")}</div>
        {errors}
      </div>
    )
  }

  if (terms.declined !== null) {
    const last = terms.version >= MAX_DRAFTS
    return (
      <div>
        <p className="quiet">
          {who(terms.declined.by)} said no to draft {terms.version} of its terms on{" "}
          {dateOf(terms.declined.at_ns)}.{" "}
          {last
            ? "That was the last draft this request takes: close it, and write the request again."
            : "Your agent may write them again."}
        </p>
        {errors}
      </div>
    )
  }

  if (tokens === undefined || accounts === undefined) {
    return <p className="quiet">Reading the terms your agent wrote…</p>
  }
  const read: DraftRead =
    tokens.length === 0
      ? { error: "the wallet could not read the ledger of any token it holds" }
      : readDraft(terms, tokens, accounts)
  const end = read.terms && endOf(read.terms)
  const ended = end !== undefined && end !== null && end < now
  const draft = `Draft ${terms.version} of at most ${MAX_DRAFTS}, written on ${dateOf(terms.drafted_at_ns)}.`
  if (read.error !== undefined || ended) {
    return (
      <div>
        <p className="notice">
          {read.error !== undefined
            ? `Your agent's terms cannot be read: ${read.error}.`
            : `Your agent's terms have already ended: “${read.confirmation.text}”`}
        </p>
        <p className="quiet">
          {draft} {owner && !fixed && "Say no, and your agent writes them again."}
        </p>
        <div className="buttons">{sayNo("Not what I meant")}</div>
        {errors}
      </div>
    )
  }
  return (
    <div>
      <p>
        <span className="pill pending">
          <span className="pill-mark" aria-hidden="true" />
          Terms to confirm
        </span>{" "}
        Your agent read in it: “{read.confirmation.text}”
      </p>
      <p className="quiet">
        {draft}{" "}
        {owner
          ? "Confirm them only if they are what you meant: your agent can then propose a transfer for this request, and the Approvals page checks it against them."
          : "Waiting for an owner to confirm them."}
      </p>
      {owner && !fixed && (
        <div className="buttons">
          <button
            type="button"
            className="primary"
            disabled={busy}
            onClick={() => confirm.mutate([request.id, read.confirmation])}
          >
            Confirm these terms
          </button>
          {sayNo("Not what I meant")}
        </div>
      )}
      {errors}
    </div>
  )
}

/** What the terms of the request a transfer answers say about it. */
export type TermsCheck =
  /** Not a transfer that answers a request, or not shown to this member. */
  | { tag: "none" }
  /** A transfer for a request whose agent wrote no terms, as before decision 0014. */
  | { tag: "no-terms"; request: bigint }
  | { tag: "reading" }
  | { tag: "failed"; error: unknown }
  | { tag: "unconfirmed"; request: bigint }
  | { tag: "unreadable"; request: bigint }
  | { tag: "checked"; request: bigint; text: string; fits: boolean; reasons: string[] }

/**
 * Whether a transfer fits the terms an owner confirmed for the request it
 * answers, read for a member who approves (`active`).
 */
export function useTermsCheck({
  wallet: id,
  pending,
  from,
  accounts,
  now,
  active,
}: {
  wallet: Principal
  pending: Pending
  from: Account | undefined
  accounts: Account[] | undefined
  now: bigint
  active: boolean
}): TermsCheck {
  const { client, wallet } = useWallet(id)
  const answers = active && pending.action.tag === "Transfer" ? pending.request : null
  const request = useQuery(
    client.queryOptions(wallet, "request", answers === null ? skipToken : answers),
  )
  if (answers === null) return { tag: "none" }
  if (request.isError) return { tag: "failed", error: request.error }
  if (request.data === undefined || accounts === undefined) return { tag: "reading" }
  const terms = request.data.terms
  if (terms === null) return { tag: "no-terms", request: answers }
  if (terms.confirmed === null) return { tag: "unconfirmed", request: answers }
  const confirmed = confirmedTerms(terms.confirmed)
  if (confirmed === undefined) return { tag: "unreadable", request: answers }
  return {
    tag: "checked",
    request: answers,
    text: terms.confirmed.text,
    ...checkTransfer(confirmed, pending.action, from, accounts, now),
  }
}

/** Whether the Approvals page offers Approve and Run: never for a transfer outside its terms. */
export function termsAllow(check: TermsCheck): boolean {
  return check.tag === "none" || check.tag === "no-terms" || (check.tag === "checked" && check.fits)
}

/** What the terms say about a transfer, above its buttons. */
export function TermsLine({ check }: { check: TermsCheck }) {
  switch (check.tag) {
    case "none":
      return null
    case "no-terms":
      return (
        <p className="notice">
          No terms cover this transfer: your agent proposed it before writing the terms of request{" "}
          {String(check.request)}.
        </p>
      )
    case "reading":
      return <p className="quiet">Reading the terms you confirmed…</p>
    case "failed":
      return (
        <p className="notice">
          The terms could not be read, so Approve waits: {failureText(check.error)}
        </p>
      )
    case "unconfirmed":
      return (
        <p className="notice">
          No owner has confirmed the terms of request {String(check.request)}, so Approve waits.
        </p>
      )
    case "unreadable":
      return (
        <p className="notice">
          The confirmed terms of request {String(check.request)} cannot be read, so this transfer
          cannot be checked against them. Reject it, and write the request again.
        </p>
      )
    case "checked":
      return (
        <div>
          <p>
            <span className={`pill ${check.fits ? "confirmed" : "failed"}`}>
              <span className="pill-mark" aria-hidden="true" />
              {check.fits ? "Within the terms" : "Outside the terms"}
            </span>{" "}
            Confirmed terms: “{check.text}”
          </p>
          {!check.fits && (
            <>
              <ul className="details">
                {check.reasons.map((reason) => (
                  <li key={reason}>{reason}</li>
                ))}
              </ul>
              <p className="quiet">
                Approve and Run are not offered for a transfer outside the terms. Reject it, and ask
                your agent again.
              </p>
            </>
          )}
        </div>
      )
  }
}
