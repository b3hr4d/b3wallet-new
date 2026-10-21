import type { Principal } from "@candid-core/schema"
import { useClient } from "@ic-reactor/react"
import { actor, type Actor } from "./generated/wallet.ts"

/**
 * The wallet the page belongs to, with this render's client. Its reads go out
 * as replicated calls, which the subnet certifies, rather than as queries one
 * node answers.
 */
export function useWallet(id: Principal) {
  const client = useClient()
  return { client, wallet: client.canister<Actor>(actor, { id, certified: true }) }
}

/** A time the wallet reports, in nanoseconds since 1970, as the browser shows it. */
export function dateOf(ns: bigint): string {
  return new Date(Number(ns / 1_000_000n)).toLocaleString()
}
