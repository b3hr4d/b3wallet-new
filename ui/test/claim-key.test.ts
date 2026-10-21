import assert from "node:assert/strict"
import { createPrivateKey, createPublicKey, sign, verify } from "node:crypto"
import { test } from "node:test"
import { claimKeyOf, claimLinkFragment, claimPublicKey, signClaim } from "../src/claim-key.ts"

const seed = Uint8Array.from({ length: 32 }, (_, i) => i * 7 + 1)
const wallet = "efadq-gl777-77774-aaaba-cai"
const caller = "2vxsx-fae"

// The PKCS #8 encoding of an Ed25519 seed, so Node's own Ed25519 checks ours.
const pkcs8 = (bytes: Uint8Array) =>
  createPrivateKey({
    key: Buffer.concat([Buffer.from("302e020100300506032b657004220420", "hex"), bytes]),
    format: "der",
    type: "pkcs8",
  })

test("a claim link's fragment is the seed in base64url without padding", () => {
  const fragment = claimLinkFragment(seed)
  assert.equal(fragment.length, 43)
  assert.equal(fragment, Buffer.from(seed).toString("base64url"))
  assert.deepEqual(claimKeyOf(fragment), seed)
  assert.deepEqual(claimKeyOf(`#${fragment}`), seed)
})

test("the fragment try-local.sh prints from a PEM opens on the page", () => {
  const key = pkcs8(seed)
  const fragment = key.export({ format: "der", type: "pkcs8" }).subarray(-32).toString("base64url")
  assert.deepEqual(claimKeyOf(fragment), seed)
})

/** The same 32 bytes, with an unused low bit of the last character set. */
function sameBytesOtherText(fragment: string): string {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
  const last = alphabet[alphabet.indexOf(fragment.slice(-1)) ^ 1] ?? ""
  const text = fragment.slice(0, -1) + last
  assert.deepEqual(
    new Uint8Array(Buffer.from(text, "base64url")),
    new Uint8Array(Buffer.from(fragment, "base64url")),
  )
  return text
}

test("anything else is no claim key", () => {
  const fragment = claimLinkFragment(seed)
  for (const text of [
    "",
    "#",
    fragment.slice(1),
    `${fragment}A`,
    `${fragment}=`,
    fragment.replace(/.$/, "+"),
    sameBytesOtherText(fragment),
  ]) {
    assert.equal(claimKeyOf(text), undefined, text)
  }
})

test("the public key is the seed's Ed25519 public key", () => {
  const spki = createPublicKey(pkcs8(seed)).export({ format: "der", type: "spki" })
  assert.deepEqual(claimPublicKey(seed), new Uint8Array(spki.subarray(-32)))
})

test("the claim signature signs the wallet and the caller, as the wallet checks", () => {
  const message = Buffer.from(`claim:${wallet}:${caller}`)
  const signature = signClaim(seed, wallet, caller)
  assert.ok(verify(null, message, createPublicKey(pkcs8(seed)), signature))
  assert.deepEqual(signature, new Uint8Array(sign(null, message, pkcs8(seed))))
  assert.ok(
    !verify(null, Buffer.from(`claim:${wallet}:aaaaa-aa`), createPublicKey(pkcs8(seed)), signature),
  )
})
