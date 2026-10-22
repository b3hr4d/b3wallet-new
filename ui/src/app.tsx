import { Live } from "./live.tsx"
import type { Page } from "./page.ts"
import { useInBrowser } from "./in-browser.ts"

/**
 * Every page: prerendered at build with the parts that are the same for
 * every visitor, then hydrated. What needs the wallet's address, the network
 * or the user renders only in the browser, from verified calls.
 */
export function App({ page }: { page: Page }) {
  const inBrowser = useInBrowser()
  return (
    <>
      <header className="bar">
        <a className="brand" href="/">
          Wallet
        </a>
        <nav className="nav">
          <a href="/send">Send</a>
          <a href="/approvals">Approvals</a>
        </nav>
      </header>
      <main className="page">
        {page === "missing" ? (
          <Missing />
        ) : inBrowser ? (
          <Live page={page} />
        ) : (
          <p className="quiet">Loading…</p>
        )}
      </main>
    </>
  )
}

function Missing() {
  return (
    <section>
      <h1>Not found</h1>
      <p>
        This wallet has no page here. <a href="/">Go to its start page.</a>
      </p>
    </section>
  )
}
