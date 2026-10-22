/** The pages the wallet serves, each prerendered into its own file. */
export const PAGES = [
  { page: "home", file: "index.html", title: "B3Wallet" },
  { page: "send", file: "send.html", title: "Send · B3Wallet" },
  { page: "receive", file: "receive.html", title: "Receive · B3Wallet" },
  { page: "account", file: "account.html", title: "Account · B3Wallet" },
  { page: "members", file: "members.html", title: "Members · B3Wallet" },
  { page: "notifications", file: "notifications.html", title: "Notifications · B3Wallet" },
  { page: "settings", file: "settings.html", title: "Settings · B3Wallet" },
  { page: "approvals", file: "approvals.html", title: "Approvals · B3Wallet" },
  { page: "claim", file: "claim.html", title: "Claim your wallet · B3Wallet" },
  { page: "missing", file: "404.html", title: "Not found · B3Wallet" },
] as const

export type Page = (typeof PAGES)[number]["page"]

/** The page a prerendered file names in its root's `data-page`. */
export function pageNamed(name: string | undefined): Page {
  return PAGES.find((entry) => entry.page === name)?.page ?? "missing"
}
