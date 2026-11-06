import { useState } from "react"
import { Live } from "./live.tsx"
import type { Page } from "./logic/page.ts"
import { B3Mark, Loader, Lockup } from "./parts/brand.tsx"
import { GearIcon } from "./parts/icons.tsx"
import { useInBrowser } from "./parts/in-browser.ts"
import { HomeOutline } from "./parts/outline.tsx"

/**
 * Every page: prerendered at build with the parts that are the same for
 * every visitor, then hydrated. The frame is one bar along the top, with the
 * brand that leads to the start page, the way to settings and who is signed
 * in, over the page itself. The start page is where a member does
 * everything day to day (decision 0017), so there is no menu of pages. What
 * needs the wallet's address, the network or the user renders only in the
 * browser, from verified calls: the page, and the account in the bar.
 */
export function App({ page }: { page: Page }) {
  const inBrowser = useInBrowser()
  const [account, setAccount] = useState<HTMLElement | null>(null)
  return (
    <div className="shell">
      <header className="spine">
        <a className="brand" href="/" aria-label="B3Wallet, the start page">
          <Lockup />
        </a>
        <div className="spine-end">
          <a
            className="gear"
            href="/settings"
            aria-label="Settings"
            title="Settings"
            aria-current={page === "settings" || page === "members" ? "page" : undefined}
          >
            <GearIcon />
          </a>
          <div className="account-slot" ref={setAccount} />
        </div>
      </header>
      <div className="stage">
        <main className="page">
          {page === "missing" ? (
            <Missing />
          ) : inBrowser ? (
            <Live page={page} account={account} />
          ) : (
            <Loading page={page} />
          )}
        </main>
        <footer className="foot">
          <span className="foot-brand">
            <B3Mark />
            B3Wallet, by B3Pay
          </span>
          <span>
            This wallet is its own only controller. Its pages come certified from its signed
            release, and this browser verifies every value it reads.
          </span>
        </footer>
      </div>
    </div>
  )
}

/** What a page shows until the app runs: the start page's outline, or the loader. */
function Loading({ page }: { page: Exclude<Page, "missing"> }) {
  return page === "home" ? (
    <HomeOutline />
  ) : (
    <div className="column page-loader">
      <Loader size={40} />
      <p>Loading…</p>
    </div>
  )
}

function Missing() {
  return (
    <section className="column">
      <div className="card empty">
        <p className="empty-code">404</p>
        <h1>Not found</h1>
        <p className="quiet">This wallet has no page here.</p>
        <a className="button" href="/">
          Go to its start page
        </a>
      </div>
    </section>
  )
}
