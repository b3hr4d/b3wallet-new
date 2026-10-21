import { sha256 } from "@noble/hashes/sha2.js"

/**
 * The code a join request shows on the device that asks and on the owner's
 * Approvals page: eight digits from the SHA-256 of the asking principal's
 * text. The owner approves the request whose code the new device shows, so a
 * request someone else sent at the same time is not approved by mistake.
 */
export function joinCode(principal: string): string {
  const hash = sha256(new TextEncoder().encode(principal))
  const number = new DataView(hash.buffer, hash.byteOffset, 4).getUint32(0) % 100_000_000
  const digits = String(number).padStart(8, "0")
  return `${digits.slice(0, 4)} ${digits.slice(4)}`
}
