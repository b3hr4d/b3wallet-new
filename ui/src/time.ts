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
