import type { Principal } from "@candid-core/schema"
import { createClient } from "@ic-reactor/core"
import { ReactorProvider } from "@ic-reactor/react"
import { AuthClient } from "@icp-sdk/auth/client"
import { Approvals } from "./approvals.tsx"
import { Claim } from "./claim.tsx"
import { Home } from "./home.tsx"
import { where } from "./host.ts"
import type { Page } from "./page.ts"
import { Send } from "./send.tsx"

/**
 * The part of a page that calls the wallet. Calls go to mainnet's API, or on
 * icp-cli's local network to the page's own origin, the only hosts the CSP
 * lets the page reach; the agent verifies every answer.
 */
export function Live({ page }: { page: Exclude<Page, "missing"> }) {
  const here = where(window.location.hostname)
  if (!here.ok) {
    return (
      <section>
        <h1>Wrong address</h1>
        <p>
          {here.reason}. A wallet works only at its own address, https://&lt;wallet id&gt;.icp0.io.
        </p>
      </section>
    )
  }
  return (
    <ReactorProvider
      client={() =>
        createClient({
          network: here.local ? { host: window.location.origin } : "ic",
          auth: (network) => new AuthClient(network),
        })
      }
    >
      <Wallet page={page} wallet={here.wallet} local={here.local} />
    </ReactorProvider>
  )
}

function Wallet({
  page,
  wallet,
  local,
}: {
  page: Exclude<Page, "missing">
  wallet: Principal
  local: boolean
}) {
  switch (page) {
    case "home":
      return <Home wallet={wallet} local={local} />
    case "send":
      return <Send wallet={wallet} />
    case "approvals":
      return <Approvals wallet={wallet} />
    case "claim":
      return <Claim wallet={wallet} />
  }
}
