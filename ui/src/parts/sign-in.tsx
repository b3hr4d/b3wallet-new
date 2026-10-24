import { useAuth } from "@ic-reactor/react"
import { useState } from "react"
import { failureText } from "../logic/describe.ts"
import { KeyIcon } from "./icons.tsx"

/** Runs a sign-in or sign-out, keeping why it failed. */
export function useRun() {
  const [failure, setFailure] = useState<string>()
  const run = (step: () => Promise<void>) => {
    setFailure(undefined)
    step().catch((error: unknown) => setFailure(failureText(error)))
  }
  return { run, failure }
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
