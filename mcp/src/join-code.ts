import { createHash } from "node:crypto"

/**
 * The code the wallet's Approvals page shows for a join request: eight digits
 * from the SHA-256 of the asking principal's text, as
 * ui/src/logic/join-code.ts makes them. The owner approves the request whose
 * code the agent reports.
 */
export function joinCode(principal: string): string {
  const hash = createHash("sha256").update(principal).digest()
  const digits = String(hash.readUInt32BE(0) % 100_000_000).padStart(8, "0")
  return `${digits.slice(0, 4)} ${digits.slice(4)}`
}
