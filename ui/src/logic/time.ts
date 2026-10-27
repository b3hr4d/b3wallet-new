/** A time the wallet reports, in nanoseconds since 1970, as the browser shows it. */
export function dateOf(ns: bigint): string {
  return new Date(Number(ns / 1_000_000n)).toLocaleString()
}

const UNITS: ReadonlyArray<readonly [bigint, string]> = [
  [86_400_000_000_000n, "day"],
  [3_600_000_000_000n, "hour"],
  [60_000_000_000n, "minute"],
  [1_000_000_000n, "second"],
]

/** A length of time in nanoseconds, in days down to seconds: `1 day and 12 hours`. */
export function durationText(ns: bigint): string {
  const parts: string[] = []
  let rest = ns
  for (const [size, unit] of UNITS) {
    const count = rest / size
    rest %= size
    if (count > 0n) parts.push(`${count} ${unit}${count === 1n ? "" : "s"}`)
  }
  if (parts.length === 0) return ns === 0n ? "no time" : "under a second"
  const last = parts.pop()
  return parts.length === 0 ? `${last}` : `${parts.join(", ")} and ${last}`
}

const WEEK_NS = 7n * 86_400_000_000_000n

/**
 * How long ago `ns` was at `nowNs`, in the largest whole unit: `5 minutes
 * ago`. A time over a week back is its date; one within a minute, or ahead of
 * the browser's clock, is `just now`.
 */
export function agoText(ns: bigint, nowNs: bigint): string {
  const past = nowNs - ns
  if (past >= WEEK_NS) return new Date(Number(ns / 1_000_000n)).toLocaleDateString()
  for (const [size, unit] of UNITS.slice(0, 3)) {
    const count = past / size
    if (count > 0n) return `${count} ${unit}${count === 1n ? "" : "s"} ago`
  }
  return "just now"
}

/**
 * How long is left until `endNs` at `nowNs`, in the largest whole unit:
 * `7 hours left`; `under a minute left` at the end, and `ended` once past it.
 */
export function leftText(endNs: bigint, nowNs: bigint): string {
  const left = endNs - nowNs
  if (left <= 0n) return "ended"
  for (const [size, unit] of UNITS.slice(0, 3)) {
    const count = left / size
    if (count > 0n) return `${count} ${unit}${count === 1n ? "" : "s"} left`
  }
  return "under a minute left"
}
