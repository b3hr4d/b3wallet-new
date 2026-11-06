import type { Principal } from "@candid-core/schema"
import { ReactorProvider } from "@ic-reactor/react"
import { createPortal } from "react-dom"
import { walletClient } from "./ic/client.ts"
import { where } from "./logic/host.ts"
import type { Page } from "./logic/page.ts"
import { Claim } from "./pages/claim.tsx"
import { Home } from "./pages/home.tsx"
import { MembersPage } from "./pages/members.tsx"
import { Settings } from "./pages/settings.tsx"
import { Account } from "./parts/session.tsx"

/**
 * The part of a page that calls the wallet, with the account it renders into
 * the bar's slot. `walletClient` picks where calls go; the agent verifies
 * every answer.
 */
export function Live({
  page,
  account,
}: {
  page: Exclude<Page, "missing">
  account: HTMLElement | null
}) {
  const here = where(window.location.hostname)
  if (!here.ok) {
    return (
      <section className="column">
        <div className="card empty">
          <h1>Wrong address</h1>
          <p>
            {here.reason}. A wallet works only at its own address, https://&lt;wallet
            id&gt;.icp0.io.
          </p>
        </div>
      </section>
    )
  }
  return (
    <ReactorProvider client={() => walletClient(here.local)}>
      {account && createPortal(<Account wallet={here.wallet} />, account)}
      <Wallet page={page} wallet={here.wallet} local={here.local} />
    </ReactorProvider>
  )
}

/**
 * The page's own part. `local` is for the settings page only, whose read of
 * the IC's certified state needs the local network's root key.
 */
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
      return <Home wallet={wallet} />
    case "members":
      return <MembersPage wallet={wallet} />
    case "settings":
      return <Settings wallet={wallet} local={local} />
    case "claim":
      return <Claim wallet={wallet} />
  }
}
