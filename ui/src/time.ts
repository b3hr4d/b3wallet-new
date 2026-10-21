/** A time the wallet reports, in nanoseconds since 1970, as the browser shows it. */
export function dateOf(ns: bigint): string {
  return new Date(Number(ns / 1_000_000n)).toLocaleString()
}
