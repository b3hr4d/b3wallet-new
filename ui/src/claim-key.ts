import { ed25519 } from "@noble/curves/ed25519.js"

/**
 * The claim link is `https://<wallet id>.icp0.io/claim#<claim key>`: the
 * claim key's 32-byte Ed25519 seed in base64url, without padding. The
 * fragment never leaves the browser.
 */
export function claimKeyOf(fragment: string): Uint8Array | undefined {
  const text = fragment.startsWith("#") ? fragment.slice(1) : fragment
  if (!/^[A-Za-z0-9_-]{43}$/.test(text)) return undefined
  const binary = atob(text.replaceAll("-", "+").replaceAll("_", "/") + "=")
  const seed = Uint8Array.from(binary, (char) => char.charCodeAt(0))
  // 43 characters hold 258 bits; a seed's text ends in a zero bit pair.
  return seed.length === 32 && claimLinkFragment(seed) === text ? seed : undefined
}

export function claimLinkFragment(seed: Uint8Array): string {
  return btoa(String.fromCharCode(...seed))
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/=+$/, "")
}

/** The claim key's public half, as the wallet stores it. */
export function claimPublicKey(seed: Uint8Array): Uint8Array {
  return ed25519.getPublicKey(seed)
}

/**
 * The signature `claim` takes: the claim key over
 * `claim:<wallet id>:<caller principal>`, so it works only for the caller it
 * names and only on this wallet.
 */
export function signClaim(seed: Uint8Array, wallet: string, caller: string): Uint8Array {
  return ed25519.sign(new TextEncoder().encode(`claim:${wallet}:${caller}`), seed)
}
