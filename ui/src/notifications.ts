import type { AuditEntry } from "./generated/wallet.ts"

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

const listeners = new Set<() => void>()

/** What this browser keeps under `key`, if it keeps anything it can read. */
export function readSeen(key: string): bigint | undefined {
  try {
    const text = window.localStorage.getItem(key)
    return text !== null && /^\d+$/.test(text) ? BigInt(text) : undefined
  } catch {
    return undefined
  }
}

/** Keeps `seq` under `key`, if the browser lets the page keep anything. */
export function writeSeen(key: string, seq: bigint): void {
  try {
    window.localStorage.setItem(key, String(seq))
  } catch {
    // A private window or blocked storage: everything stays unseen.
  }
  listeners.forEach((listener) => listener())
}

/** Calls `listener` when this page or another tab changes what was seen. */
export function subscribeSeen(listener: () => void): () => void {
  listeners.add(listener)
  window.addEventListener("storage", listener)
  return () => {
    listeners.delete(listener)
    window.removeEventListener("storage", listener)
  }
}
