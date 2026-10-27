import { isPrincipal, principal, type Principal } from "@candid-core/schema"

/** Which wallet serves this page, and on which network. */
export type Here = { ok: true; wallet: Principal; local: boolean } | { ok: false; reason: string }

/**
 * The wallet serves its own pages, at `https://<id>.icp0.io` on mainnet and
 * `http://<id>.localhost:<port>` on icp-cli's local network, so the first
 * label of the host names it. Any other address could be a page that only
 * looks like the wallet's, or one whose Internet Identity principals differ
 * from the wallet's own address, so the page works only there.
 */
export function where(hostname: string): Here {
  const [first = "", ...rest] = hostname.toLowerCase().split(".")
  const domain = rest.join(".")
  if (domain !== "icp0.io" && domain !== "localhost") {
    return { ok: false, reason: `${hostname} is not a wallet's own address` }
  }
  if (!isPrincipal(first)) {
    return { ok: false, reason: `${first} is not a canister id` }
  }
  return { ok: true, wallet: principal(first), local: domain === "localhost" }
}

/**
 * A whole number the page's address names, such as the account in
 * `?account=2`, or undefined when it names none.
 */
export function numberParam(search: string, name: string): number | undefined {
  const text = new URLSearchParams(search).get(name)
  if (text === null || !/^\d{1,10}$/.test(text)) return undefined
  const value = Number(text)
  return value <= 0xffff_ffff ? value : undefined
}
