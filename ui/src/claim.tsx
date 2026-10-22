import type { Principal } from "@candid-core/schema"
import { useAuth } from "@ic-reactor/react"
import { useMutation, useQuery } from "@tanstack/react-query"
import { useEffect, useState, type ReactNode } from "react"
import { useWallet } from "./canister.ts"
import { dateOf } from "./time.ts"
import { claimKeyOf, signClaim } from "./claim-key.ts"
import { failureText } from "./describe.ts"
import { WalletMark } from "./icons.tsx"
import { Reading } from "./member.tsx"
import { SignIn } from "./session.tsx"

/**
 * The claim link's page: whoever signs in here and claims becomes the
 * wallet's first owner. The claim key signs the caller's principal in the
 * browser; only the signature leaves it.
 */
export function Claim({ wallet: id }: { wallet: Principal }) {
  // Read once; then the key leaves the address bar and the history, so a
  // screenshot, a shared address or the back button does not carry it on.
  const [seed] = useState(() => claimKeyOf(window.location.hash))
  useEffect(() => {
    if (window.location.hash) {
      window.history.replaceState(window.history.state, "", window.location.pathname)
    }
  }, [])

  const auth = useAuth()
  const { client, wallet } = useWallet(id)
  const status = useQuery(client.queryOptions(wallet, "status"))
  const claim = useMutation(client.mutationOptions(wallet, "claim"))

  let body: ReactNode
  if (claim.isSuccess) {
    body = (
      <>
        <h1>The wallet is yours</h1>
        <p className="quiet">You are its first owner, signed in with this Internet Identity.</p>
        <a className="button primary" href="/">
          Go to the wallet
        </a>
      </>
    )
  } else if (status.isError) {
    body = <p className="error">{failureText(status.error)}</p>
  } else if (status.data === undefined) {
    body = <Reading>Reading the wallet…</Reading>
  } else if (status.data.claimed) {
    body = (
      <p>
        This wallet is already claimed. <a href="/">Go to the wallet.</a>
      </p>
    )
  } else if (seed === undefined) {
    body = (
      <p className="error">
        This address holds no claim key. Open the claim link you were given again, exactly as it was
        sent.
      </p>
    )
  } else {
    const expiresAt = status.data.claim_expires_at_ns
    body = (
      <>
        <p className="quiet">
          Whoever claims this wallet becomes its first owner. Sign in with the identity that should
          own it.
        </p>
        {expiresAt !== null && (
          <p className="quiet small">The claim link works until {dateOf(expiresAt)}.</p>
        )}
        <ol className="steps">
          <li className={auth.status === "signed-in" ? "step done" : "step"}>
            <span className="step-number">1</span>
            {auth.status === "signed-in" ? (
              <p>
                Signed in as <code className="principal">{auth.principal}</code>
              </p>
            ) : (
              <SignIn />
            )}
          </li>
          <li className="step">
            <span className="step-number">2</span>
            <div>
              <button
                type="button"
                className="primary"
                disabled={auth.status !== "signed-in" || claim.isPending}
                onClick={() =>
                  auth.status === "signed-in" &&
                  claim.mutate({ signature: signClaim(seed, id, auth.principal) })
                }
              >
                {claim.isPending ? "Claiming…" : "Claim the wallet"}
              </button>
              {claim.isError && <p className="error">{failureText(claim.error)}</p>}
            </div>
          </li>
        </ol>
      </>
    )
  }
  return (
    <section className="column">
      <div className="card claim">
        <WalletMark id={id} />
        {!claim.isSuccess && <h1>Claim your wallet</h1>}
        {body}
      </div>
    </section>
  )
}
