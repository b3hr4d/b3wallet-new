import { createClient } from "@ic-reactor/core"
import { AuthClient } from "@icp-sdk/auth/client"

/**
 * The client every call to the wallet and its ledgers goes through, and with
 * `facts.ts` the only code that picks where calls go: mainnet's API, or on
 * icp-cli's local network the page's own origin, the only hosts the CSP lets
 * the page reach. Members sign in with Internet Identity through `AuthClient`.
 */
export function walletClient(local: boolean) {
  return createClient({
    network: local ? { host: window.location.origin } : "ic",
    auth: (network) => new AuthClient(network),
  })
}
