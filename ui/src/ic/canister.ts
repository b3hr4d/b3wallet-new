import type { Principal } from "@candid-core/schema"
import { useClient } from "@ic-reactor/react"
import { actor, type Actor } from "../generated/wallet.ts"

/**
 * The wallet the page belongs to, with this render's client. Its reads go out
 * as replicated calls, which the subnet certifies, rather than as queries one
 * node answers.
 */
export function useWallet(id: Principal) {
  const client = useClient()
  return { client, wallet: client.canister<Actor>(actor, { id, certified: true }) }
}

/**
 * The same wallet for plain queries, which one node answers for free: only for
 * a read the wallet refuses as a replicated call, such as `guard_check`.
 */
export function useQueryWallet(id: Principal) {
  const client = useClient()
  return { client, wallet: client.canister<Actor>(actor, { id, certified: false }) }
}
