// The agent's own key: an Ed25519 key in a PKCS #8 PEM file that only its
// owner can read, made on first run. icp-cli imports the same file
// (`icp identity import agent --from-pem <file>`), so the principal it shows
// can be checked by hand.
import { randomBytes } from "node:crypto"
import { mkdir, readFile, stat, writeFile } from "node:fs/promises"
import { dirname } from "node:path"
import { Ed25519KeyIdentity } from "@icp-sdk/core/identity"

/** What every PKCS #8 Ed25519 private key starts with, before its 32-byte seed (RFC 8410). */
const PKCS8_ED25519 = Buffer.from("302e020100300506032b657004220420", "hex")
const LABEL = "PRIVATE KEY"

/** A 32-byte Ed25519 seed as a PKCS #8 PEM file's text. */
export function pemOf(seed: Uint8Array): string {
  if (seed.length !== 32) throw new RangeError("an Ed25519 seed is 32 bytes")
  const der = Buffer.concat([PKCS8_ED25519, seed]).toString("base64")
  return `-----BEGIN ${LABEL}-----\n${der}\n-----END ${LABEL}-----\n`
}

/** The seed of a PEM file `pemOf` wrote, or an error for anything else. */
export function seedOf(pem: string): Uint8Array {
  const match = new RegExp(
    `^-----BEGIN ${LABEL}-----\\s+([A-Za-z0-9+/=\\s]+?)\\s*-----END ${LABEL}-----\\s*$`,
  ).exec(pem)
  const der = match?.[1] ? Buffer.from(match[1].replace(/\s/g, ""), "base64") : undefined
  if (
    der?.length !== PKCS8_ED25519.length + 32 ||
    !der.subarray(0, PKCS8_ED25519.length).equals(PKCS8_ED25519)
  ) {
    throw new Error("the key file is not an Ed25519 private key in PKCS #8 PEM")
  }
  return new Uint8Array(der.subarray(PKCS8_ED25519.length))
}

/**
 * The identity whose key is at `path`, made there first if no file is. A key
 * file that others can read or write is refused, as ssh refuses one: someone
 * else could have read it, or put their own key there.
 */
export async function loadOrCreateKey(
  path: string,
): Promise<{ identity: Ed25519KeyIdentity; created: boolean }> {
  let created = false
  try {
    await mkdir(dirname(path), { recursive: true, mode: 0o700 })
    await writeFile(path, pemOf(randomBytes(32)), { mode: 0o600, flag: "wx" })
    created = true
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error
  }
  if (process.platform !== "win32") {
    const { mode } = await stat(path)
    if ((mode & 0o077) !== 0) {
      throw new Error(
        `others can read or write the key file ${path}: run chmod 600 on it, or delete it for a new key`,
      )
    }
  }
  const identity = Ed25519KeyIdentity.generate(seedOf(await readFile(path, "utf8")))
  return { identity, created }
}
