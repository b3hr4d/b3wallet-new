import type { Principal } from "@candid-core/schema"
import { useAuth } from "@ic-reactor/react"
import { useMutation, useQuery } from "@tanstack/react-query"
import { useEffect, useState } from "react"
import { dateOf, useWallet } from "./canister.ts"
import { claimKeyOf, signClaim } from "./claim-key.ts"
import { failureText } from "./describe.ts"
import { Session } from "./session.tsx"

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

  if (claim.isSuccess) {
    return (
      <section>
        <h1>The wallet is yours</h1>
        <p>You are its first owner, signed in with this Internet Identity.</p>
        <p>
          <a href="/">Go to the wallet</a>
        </p>
      </section>
    )
  }
  if (status.isError) {
    return (
      <section>
        <h1>Claim your wallet</h1>
        <p className="error">{failureText(status.error)}</p>
      </section>
    )
  }
  if (status.data === undefined) {
    return (
      <section>
        <h1>Claim your wallet</h1>
        <p className="quiet">Reading the wallet…</p>
      </section>
    )
  }
  if (status.data.claimed) {
    return (
      <section>
        <h1>Claim your wallet</h1>
        <p>
          This wallet is already claimed. <a href="/">Go to the wallet.</a>
        </p>
      </section>
    )
  }
  if (seed === undefined) {
    return (
      <section>
        <h1>Claim your wallet</h1>
        <p className="error">
          This address holds no claim key. Open the claim link you were given again, exactly as it
          was sent.
        </p>
      </section>
    )
  }
  const expiresAt = status.data.claim_expires_at_ns
  return (
    <section>
      <h1>Claim your wallet</h1>
      <p>
        Whoever claims this wallet becomes its first owner. Sign in with the identity that should
        own it.
      </p>
      {expiresAt !== null && (
        <p className="quiet">The claim link works until {dateOf(expiresAt)}.</p>
      )}
      <Session />
      {auth.status === "signed-in" && (
        <>
          <button
            type="button"
            className="primary"
            disabled={claim.isPending}
            onClick={() => claim.mutate({ signature: signClaim(seed, id, auth.principal) })}
          >
            {claim.isPending ? "Claiming…" : "Claim the wallet"}
          </button>
          {claim.isError && <p className="error">{failureText(claim.error)}</p>}
        </>
      )}
    </section>
  )
}
