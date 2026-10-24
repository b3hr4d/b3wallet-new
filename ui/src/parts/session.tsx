import type { Principal } from "@candid-core/schema"
import { useAuth } from "@ic-reactor/react"
import { skipToken, useQuery } from "@tanstack/react-query"
import { useEffect, useRef, useState } from "react"
import { useWallet } from "../ic/canister.ts"
import { failureText, shortText } from "../logic/describe.ts"
import { healthOf } from "../logic/health.ts"
import { CheckList } from "./checks.tsx"
import { CopyButton } from "./copy.tsx"
import { ArrowIcon } from "./icons.tsx"
import { useMe } from "./member.tsx"
import { useNowNs } from "./now.ts"
import { useRun } from "./sign-in.tsx"

/**
 * Who is signed in, in the spine on every page. Signed in, it is the
 * wallet's health: a dot that is the logo's node while all is well, sends out
 * a ring when something needs you and turns red when something is wrong, and
 * opens to say why, with the inbox and the sign-out. Internet Identity gives each site its
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
        <Health wallet={wallet} principal={auth.principal} onSignOut={() => run(auth.signOut)} />
      ) : (
        <button type="button" className="primary small" onClick={() => run(auth.signIn)}>
          {auth.status === "expired" ? "Sign in again" : "Sign in"}
        </button>
      )}
    </>
  )
}

/**
 * The wallet's health, as the chip, the rail and the inbox show it. It reads
 * the status and who you are, which every page reads, and for a member the
 * open actions, which the start page and the rail read too.
 */
export function useHealth(id: Principal) {
  const auth = useAuth()
  const { status, me } = useMe(id)
  const { client, wallet } = useWallet(id)
  const member = me.isSuccess ? me.data : undefined
  const pending = useQuery(client.queryOptions(wallet, "pending", member ? undefined : skipToken))
  const nowNs = useNowNs()
  return healthOf({
    status: status.data,
    statusFailure: status.isError ? failureText(status.error) : undefined,
    me: member,
    pending: pending.data,
    sessionEndsNs:
      auth.status === "signed-in" && auth.expiresAt !== undefined
        ? BigInt(auth.expiresAt) * 1_000_000n
        : undefined,
    nowNs,
  })
}

/** The health chip and its panel, which opens to every check and the sign-out. */
function Health({
  wallet: id,
  principal,
  onSignOut,
}: {
  wallet: Principal
  principal: string
  onSignOut: () => void
}) {
  const health = useHealth(id)
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
    <div className="account-menu" ref={box}>
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
          <a className="health-news" href="/notifications">
            <span>Open the inbox</span>
            <ArrowIcon />
          </a>
          <CheckList checks={health.checks} />
          <div className="health-foot">
            <p className="field-label">Signed in as</p>
            <p className="address-box">
              <code>{principal}</code>
              <CopyButton text={principal} label="Copy your principal" />
            </p>
            <div className="buttons">
              <a className="button small" href="/settings">
                Settings
              </a>
              <button type="button" className="ghost small" onClick={onSignOut}>
                Sign out
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
