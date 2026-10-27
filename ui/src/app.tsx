import { Fragment, useState, type ReactNode } from "react"
import { Live, type Slots } from "./live.tsx"
import type { Page, Path } from "./logic/page.ts"
import { B3Mark, Loader, Lockup } from "./parts/brand.tsx"
import {
  ApproveIcon,
  GearIcon,
  InboxIcon,
  OverviewIcon,
  PeopleIcon,
  ReceiveIcon,
  SendIcon,
} from "./parts/icons.tsx"
import { useInBrowser } from "./parts/in-browser.ts"
import { HomeOutline, RailOutline } from "./parts/outline.tsx"

type Link = {
  page: Page
  href: Path
  label: string
  icon: ReactNode
  /** A count the app fills in: what waits for you, or what is new. */
  count?: "approvals" | "inbox"
  /** In the sidebar only: the bar and the tab bar reach it through the gear. */
  extra?: boolean
}

const LINKS: readonly Link[] = [
  { page: "home", href: "/", label: "Overview", icon: <OverviewIcon /> },
  { page: "send", href: "/send", label: "Send", icon: <SendIcon /> },
  { page: "receive", href: "/receive", label: "Receive", icon: <ReceiveIcon /> },
  {
    page: "approvals",
    href: "/approvals",
    label: "Approvals",
    icon: <ApproveIcon />,
    count: "approvals",
  },
  {
    page: "notifications",
    href: "/notifications",
    label: "Inbox",
    icon: <InboxIcon />,
    count: "inbox",
  },
  { page: "members", href: "/members", label: "Members", icon: <PeopleIcon />, extra: true },
  { page: "settings", href: "/settings", label: "Settings", icon: <GearIcon />, extra: true },
]

/**
 * Every page: prerendered at build with the parts that are the same for
 * every visitor, then hydrated. The frame is a spine of pages (a sidebar on
 * a desktop, a bar on a tablet, a tab bar on a phone), the page itself, and
 * on a wide screen a rail of what needs you. What needs the wallet's
 * address, the network or the user renders only in the browser, from
 * verified calls, into the elements the frame leaves empty for it: the
 * account in the spine, the counts beside two pages, and the rail.
 */
export function App({ page }: { page: Page }) {
  const inBrowser = useInBrowser()
  const [account, setAccount] = useState<HTMLElement | null>(null)
  const [rail, setRail] = useState<HTMLElement | null>(null)
  const [approvals, setApprovals] = useState<HTMLElement | null>(null)
  const [inbox, setInbox] = useState<HTMLElement | null>(null)
  const slots: Slots = { account, rail, approvals, inbox }
  return (
    <div className={page === "missing" ? "shell no-rail" : "shell"}>
      <header className="spine">
        <a className="brand" href="/" aria-label="B3Wallet, the start page">
          <Lockup />
        </a>
        <nav className="nav" aria-label="Pages">
          {LINKS.map((link, index) => (
            <Fragment key={link.page}>
              {link.extra && !LINKS[index - 1]?.extra && (
                <span className="nav-group" aria-hidden="true">
                  Wallet
                </span>
              )}
              <a
                className={link.extra ? "nav-extra" : undefined}
                href={link.href}
                aria-current={
                  link.page === page || (link.page === "home" && page === "account")
                    ? "page"
                    : undefined
                }
              >
                {link.icon}
                <span className="nav-label">{link.label}</span>
                {link.count && (
                  <span
                    className="nav-slot"
                    ref={link.count === "approvals" ? setApprovals : setInbox}
                  />
                )}
              </a>
            </Fragment>
          ))}
        </nav>
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
        <p className="spine-note">
          <B3Mark />
          Self-custody. Certified pages.
        </p>
      </header>
      <div className="stage">
        <main className="page">
          {page === "missing" ? (
            <Missing />
          ) : inBrowser ? (
            <Live page={page} slots={slots} />
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
      {page !== "missing" && (
        <aside className="rail" aria-label="Needs you, and what happened" ref={setRail}>
          {!inBrowser && <RailOutline />}
        </aside>
      )}
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
