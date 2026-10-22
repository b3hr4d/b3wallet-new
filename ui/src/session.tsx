import { useAuth } from "@ic-reactor/react"
import { useState } from "react"
import { failureText, shortText } from "./describe.ts"
import { KeyIcon } from "./icons.tsx"

/** Runs a sign-in or sign-out, keeping why it failed. */
function useRun() {
  const [failure, setFailure] = useState<string>()
  const run = (step: () => Promise<void>) => {
    setFailure(undefined)
    step().catch((error: unknown) => setFailure(failureText(error)))
  }
  return { run, failure }
}

/**
 * Who is signed in, at the end of the bar on every page, with the button that
 * changes it. Internet Identity gives each site its own principal, so the
 * identity this page signs in with belongs to this wallet's address only.
 */
export function Account() {
  const auth = useAuth()
  const { run, failure } = useRun()
  return (
    <>
      {failure && <span className="error bar-error">{failure}</span>}
      {auth.status === "signed-in" ? (
        <>
          <span className="who" title={auth.principal}>
            <span className="avatar tiny" aria-hidden="true" />
            <code>{shortText(auth.principal)}</code>
          </span>
          <button type="button" className="ghost small" onClick={() => run(auth.signOut)}>
            Sign out
          </button>
        </>
      ) : (
        <button type="button" className="primary small" onClick={() => run(auth.signIn)}>
          {auth.status === "expired" ? "Sign in again" : "Sign in"}
        </button>
      )}
    </>
  )
}

/** The sign-in inside a page, for what someone must sign in to see or do. */
export function SignIn() {
  const auth = useAuth()
  const { run, failure } = useRun()
  return (
    <div className="sign-in">
      {auth.status === "expired" && <p className="quiet">Your session ended.</p>}
      <button type="button" className="primary" onClick={() => run(auth.signIn)}>
        <KeyIcon />
        Sign in with Internet Identity
      </button>
      {failure && <p className="error">{failure}</p>}
    </div>
  )
}
