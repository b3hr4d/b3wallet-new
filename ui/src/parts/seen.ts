/**
 * Which audit entries this browser has seen, kept in its local storage per
 * wallet and principal: a convenience, never a record (logic/notifications.ts
 * says why).
 */

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
