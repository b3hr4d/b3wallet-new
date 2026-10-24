import type { Principal } from "@candid-core/schema"
import { ReactorProvider } from "@ic-reactor/react"
import { createPortal } from "react-dom"
import { walletClient } from "./ic/client.ts"
import { where } from "./logic/host.ts"
import type { Page } from "./logic/page.ts"
import { AccountPage } from "./pages/account.tsx"
import { Approvals } from "./pages/approvals.tsx"
import { Claim } from "./pages/claim.tsx"
import { Home } from "./pages/home.tsx"
import { MembersPage } from "./pages/members.tsx"
import { NotificationsPage } from "./pages/notifications.tsx"
import { Receive } from "./pages/receive.tsx"
import { Send } from "./pages/send.tsx"
import { Settings } from "./pages/settings.tsx"
import { ApprovalsCount, InboxCount, Rail } from "./parts/rail.tsx"
import { Account } from "./parts/session.tsx"

/** The elements the frame leaves empty for what only the browser knows. */
export type Slots = {
  account: HTMLElement | null
  rail: HTMLElement | null
  approvals: HTMLElement | null
  inbox: HTMLElement | null
}

/**
 * The part of a page that calls the wallet, with what it renders into the
 * frame's slots: the account, the counts beside Approvals and Inbox, and the
 * rail. `walletClient` picks where calls go; the agent verifies every
 * answer.
 */
export function Live({ page, slots }: { page: Exclude<Page, "missing">; slots: Slots }) {
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
      {slots.account && createPortal(<Account wallet={here.wallet} />, slots.account)}
      {slots.approvals && createPortal(<ApprovalsCount wallet={here.wallet} />, slots.approvals)}
      {slots.inbox && createPortal(<InboxCount wallet={here.wallet} />, slots.inbox)}
      {slots.rail && createPortal(<Rail wallet={here.wallet} page={page} />, slots.rail)}
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
