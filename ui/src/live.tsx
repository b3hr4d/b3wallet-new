import type { Principal } from "@candid-core/schema"
import { createClient } from "@ic-reactor/core"
import { ReactorProvider } from "@ic-reactor/react"
import { AuthClient } from "@icp-sdk/auth/client"
import { createPortal } from "react-dom"
import { AccountPage } from "./account.tsx"
import { Approvals } from "./approvals.tsx"
import { Claim } from "./claim.tsx"
import { Home } from "./home.tsx"
import { where } from "./host.ts"
import type { Page } from "./page.ts"
import { MembersPage } from "./members.tsx"
import { NotificationsPage } from "./notifications-page.tsx"
import { Receive } from "./receive.tsx"
import { Send } from "./send.tsx"
import { Settings } from "./settings.tsx"
import { Account } from "./session.tsx"

/**
 * The part of a page that calls the wallet, with the account it renders into
 * the bar's `account` element. Calls go to mainnet's API, or on icp-cli's
 * local network to the page's own origin, the only hosts the CSP lets the
 * page reach; the agent verifies every answer.
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
    <ReactorProvider
      client={() =>
        createClient({
          network: here.local ? { host: window.location.origin } : "ic",
          auth: (network) => new AuthClient(network),
        })
      }
    >
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
    case "send":
      return <Send wallet={wallet} />
    case "receive":
      return <Receive wallet={wallet} />
    case "account":
      return <AccountPage wallet={wallet} />
    case "members":
      return <MembersPage wallet={wallet} />
    case "notifications":
      return <NotificationsPage wallet={wallet} />
    case "settings":
      return <Settings wallet={wallet} local={local} />
    case "approvals":
      return <Approvals wallet={wallet} />
    case "claim":
      return <Claim wallet={wallet} />
  }
}
