import { useAuth } from "@ic-reactor/react"
import { useState } from "react"
import { failureText } from "./describe.ts"

/**
 * Who is signed in, with the button that changes it. Internet Identity gives
 * each site its own principal, so the identity this page signs in with
 * belongs to this wallet's address only.
 */
export function Session() {
  const auth = useAuth()
  const [failure, setFailure] = useState<string>()
  const run = (step: () => Promise<void>) => {
    setFailure(undefined)
    step().catch((error: unknown) => setFailure(failureText(error)))
  }
  return (
    <section className="session">
      {auth.status === "signed-in" ? (
        <>
          <p>
            Signed in as <code className="principal">{auth.principal}</code>
          </p>
          <button type="button" onClick={() => run(auth.signOut)}>
            Sign out
          </button>
        </>
      ) : (
        <>
          {auth.status === "expired" && <p>Your session ended.</p>}
          <button type="button" className="primary" onClick={() => run(auth.signIn)}>
            Sign in with Internet Identity
          </button>
        </>
      )}
      {failure && <p className="error">{failure}</p>}
    </section>
  )
}
