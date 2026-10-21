/** The pages the wallet serves, each prerendered into its own file. */
export const PAGES = [
  { page: "home", file: "index.html", title: "Wallet" },
  { page: "send", file: "send.html", title: "Send" },
  { page: "approvals", file: "approvals.html", title: "Approvals" },
  { page: "claim", file: "claim.html", title: "Claim your wallet" },
  { page: "missing", file: "404.html", title: "Not found" },
] as const

export type Page = (typeof PAGES)[number]["page"]

/** The page a prerendered file names in its root's `data-page`. */
export function pageNamed(name: string | undefined): Page {
  return PAGES.find((entry) => entry.page === name)?.page ?? "missing"
}
