import { useSyncExternalStore } from "react"

let now = Date.now()

function subscribe(onChange: () => void): () => void {
  now = Date.now()
  const timer = setInterval(() => {
    now = Date.now()
    onChange()
  }, 10_000)
  return () => clearInterval(timer)
}

/**
 * The browser's time in nanoseconds since 1970, as the wallet counts it,
 * renewed every ten seconds: when a timelock has passed. The wallet decides
 * by its own clock, and says so when it refuses.
 */
export function useNowNs(): bigint {
  const ms = useSyncExternalStore(
    subscribe,
    () => now,
    () => now,
  )
  return BigInt(ms) * 1_000_000n
}
