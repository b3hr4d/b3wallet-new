import type { AuditEntry } from "../generated/wallet.ts"

/**
 * Notifications are what the wallet already records: the health dot's
 * checks, and the audit log's entries since you last looked. Which entries
 * you have seen is kept in this browser, per wallet and principal: a
 * convenience, never a record. Nothing the wallet does depends on it, and a
 * private window or a cleared site starts again with nothing new.
 */

/** Where this browser keeps the newest entry `principal` has seen of `wallet`. */
export function seenKey(wallet: string, principal: string): string {
  return `b3wallet.seen.${wallet}.${principal}`
}

/**
 * The entries `me` has not seen: newer than `seen`, and done by someone
 * else or by the wallet itself, since what you did yourself is no news. With
 * nothing seen yet, on a first visit, nothing is new.
 */
export function unreadOf(
  entries: readonly AuditEntry[],
  seen: bigint | undefined,
  me: string,
): AuditEntry[] {
  if (seen === undefined) return []
  return entries.filter((entry) => entry.seq > seen && entry.by !== me)
}

/** The newest sequence number in `entries`, or 0 for none. */
export function newestOf(entries: readonly AuditEntry[]): bigint {
  return entries.reduce((newest, entry) => (entry.seq > newest ? entry.seq : newest), 0n)
}
