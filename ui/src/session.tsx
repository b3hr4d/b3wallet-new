import type { Principal } from "@candid-core/schema"
import { useAuth } from "@ic-reactor/react"
import { skipToken, useQuery } from "@tanstack/react-query"
import { useEffect, useRef, useState } from "react"
import { useWallet } from "./canister.ts"
import { CopyButton } from "./copy.tsx"
import { failureText, shortText } from "./describe.ts"
import { healthOf } from "./health.ts"
import { useMe } from "./member.tsx"
import { useNowNs } from "./now.ts"
import { useRun } from "./sign-in.tsx"

/**
 * Who is signed in, at the end of the bar on every page. Signed in, it is the
 * wallet's health: a dot that is the logo's node while all is well, sends out
 * a ring when something needs you and turns red when something is wrong, and
 * opens to say why, with the sign-out. Internet Identity gives each site its
 * own principal, so the identity this page signs in with belongs to this
 * wallet's address only.
 */
export function Account({ wallet }: { wallet: Principal }) {
  const auth = useAuth()
  const { run, failure } = useRun()
  return (
    <>
      {failure && <span className="error bar-error">{failure}</span>}
      {auth.status === "signed-in" ? (
        <Health
          wallet={wallet}
          principal={auth.principal}
          expiresAt={auth.expiresAt}
          onSignOut={() => run(auth.signOut)}
        />
      ) : (
        <button type="button" className="primary small" onClick={() => run(auth.signIn)}>
          {auth.status === "expired" ? "Sign in again" : "Sign in"}
        </button>
      )}
    </>
  )
}

/**
 * The health chip and its panel. It reads what the page reads (the status and
 * who you are, and for a member the open actions), so the dot costs no call
 * the page does not already make, but on the send page.
 */
function Health({
  wallet: id,
  principal,
  expiresAt,
  onSignOut,
}: {
  wallet: Principal
  principal: string
  expiresAt: number | undefined
  onSignOut: () => void
}) {
  const { status, me } = useMe(id)
  const { client, wallet } = useWallet(id)
  const member = me.isSuccess ? me.data : undefined
  const pending = useQuery(client.queryOptions(wallet, "pending", member ? undefined : skipToken))
  const nowNs = useNowNs()
  const health = healthOf({
    status: status.data,
    statusFailure: status.isError ? failureText(status.error) : undefined,
    me: member,
    pending: pending.data,
    sessionEndsNs: expiresAt === undefined ? undefined : BigInt(expiresAt) * 1_000_000n,
    nowNs,
  })

  const [open, setOpen] = useState(false)
  const box = useRef<HTMLDivElement>(null)
  // Closes on a press outside the chip and its panel, and on Escape.
  useEffect(() => {
    if (!open) return
    const press = (event: PointerEvent) => {
      if (event.target instanceof Node && !box.current?.contains(event.target)) setOpen(false)
    }
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false)
    }
    document.addEventListener("pointerdown", press)
    document.addEventListener("keydown", key)
    return () => {
      document.removeEventListener("pointerdown", press)
      document.removeEventListener("keydown", key)
    }
  }, [open])

  return (
    <div className="account" ref={box}>
      <button
        type="button"
        className={`who ${health.level}`}
        aria-expanded={open}
        aria-controls="health"
        aria-label={`Signed in as ${shortText(principal)}. Wallet health: ${health.summary}.`}
        title={health.summary}
        onClick={() => setOpen(!open)}
      >
        <span className="health-dot" aria-hidden="true" />
        <code>{shortText(principal)}</code>
      </button>
      {open && (
        <div id="health" className="health-panel" role="dialog" aria-label="Wallet health">
          <p className="health-head">
            <span className={`health-mark ${health.level}`} aria-hidden="true" />
            {health.summary}
          </p>
          <ul className="health-list">
            {health.checks.map((check) => (
              <li key={check.title} className="health-item">
                <span className={`health-mark ${check.level}`} aria-hidden="true" />
                <div>
                  <p className="health-title">
                    {check.href ? <a href={check.href}>{check.title}</a> : check.title}
                    {check.level !== "ok" && (
                      <span className="visually-hidden">
                        {check.level === "problem" ? " (a problem)" : " (needs attention)"}
                      </span>
                    )}
                  </p>
                  <p className="health-text">{check.text}</p>
                </div>
              </li>
            ))}
          </ul>
          <div className="health-foot">
            <p className="field-label">Signed in as</p>
            <p className="address-box">
              <code>{principal}</code>
              <CopyButton text={principal} label="Copy your principal" />
            </p>
            <button type="button" className="small" onClick={onSignOut}>
              Sign out
            </button>
          </div>
        </div>
      )}
    </div>
  )
}
