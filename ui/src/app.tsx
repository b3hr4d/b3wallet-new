import { useState, type ReactNode } from "react"
import { ApproveIcon, Logo, OverviewIcon, SendIcon } from "./icons.tsx"
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
          <a className="brand" href="/">
            <Logo />
            <span>Wallet</span>
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
        <span className="foot-dot" aria-hidden="true" />
        <span>
          Pages certified from the wallet&apos;s signed release. Every value read by a call this
          browser verifies.
        </span>
      </footer>
    </>
  )
}

/** What a page shows until the app runs: the start page's outline, or a spinner. */
function Loading({ page }: { page: Exclude<Page, "missing"> }) {
  return page === "home" ? (
    <HomeOutline />
  ) : (
    <div className="column">
      <p className="loading">
        <span className="spinner" aria-hidden="true" />
        Loading…
      </p>
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
