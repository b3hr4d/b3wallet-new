import { useState, type ReactNode } from "react"
import { B3Mark, Loader, Lockup } from "./brand.tsx"
import { ApproveIcon, OverviewIcon, SendIcon } from "./icons.tsx"
import { Live } from "./live.tsx"
import { HomeOutline } from "./outline.tsx"
import type { Page } from "./page.ts"
import { useInBrowser } from "./in-browser.ts"

const LINKS: ReadonlyArray<{ page: Page; href: string; label: string; icon: ReactNode }> = [
  { page: "home", href: "/", label: "Overview", icon: <OverviewIcon /> },
  { page: "send", href: "/send", label: "Send", icon: <SendIcon /> },
  { page: "approvals", href: "/approvals", label: "Approvals", icon: <ApproveIcon /> },
]

/**
 * Every page: prerendered at build with the parts that are the same for
 * every visitor, then hydrated. What needs the wallet's address, the network
 * or the user renders only in the browser, from verified calls; the account
 * at the end of the bar renders there too, into the element it leaves empty.
 */
export function App({ page }: { page: Page }) {
  const inBrowser = useInBrowser()
  const [account, setAccount] = useState<HTMLElement | null>(null)
  return (
    <>
      <header className="bar">
        <div className="bar-inner">
          <a className="brand" href="/" aria-label="B3Wallet, the start page">
            <Lockup />
          </a>
          <nav className="nav" aria-label="Pages">
            {LINKS.map((link) => (
              <a
                key={link.page}
                href={link.href}
                aria-current={link.page === page ? "page" : undefined}
              >
                {link.icon}
                <span>{link.label}</span>
              </a>
            ))}
          </nav>
          <div className="bar-end" ref={setAccount} />
        </div>
      </header>
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
          This wallet is its own only controller. Its pages come certified from its signed release,
          and this browser verifies every value it reads.
        </span>
      </footer>
    </>
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
