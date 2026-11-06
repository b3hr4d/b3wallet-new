/**
 * The pages the wallet serves, each prerendered into its own file: the start
 * page, where a member does everything day to day (decision 0017); settings,
 * with members a page of its own, for setting the wallet up; and the claim.
 */
export const PAGES = [
  { page: "home", file: "index.html", title: "B3Wallet" },
  { page: "settings", file: "settings.html", title: "Settings · B3Wallet" },
  { page: "members", file: "members.html", title: "Members · B3Wallet" },
  { page: "claim", file: "claim.html", title: "Claim your wallet · B3Wallet" },
  { page: "missing", file: "404.html", title: "Not found · B3Wallet" },
] as const

export type Page = (typeof PAGES)[number]["page"]

/**
 * Where a link may go: a page of the wallet's own, by its path from the root,
 * or a fragment of this one. The pages link nowhere else.
 */
export type Path = `/${string}` | `#${string}`

/** The page a prerendered file names in its root's `data-page`. */
export function pageNamed(name: string | undefined): Page {
  return PAGES.find((entry) => entry.page === name)?.page ?? "missing"
}
