/**
 * The 32 bytes a SHA-256 of 64 hex digits names, as an owner pastes it from
 * recovery-tool, or undefined for anything else.
 */
export function hashOf(text: string): Uint8Array | undefined {
  const hex = text.trim().toLowerCase()
  if (!/^[0-9a-f]{64}$/.test(hex)) return undefined
  return Uint8Array.from({ length: 32 }, (_, index) =>
    parseInt(hex.slice(2 * index, 2 * index + 2), 16),
  )
}
