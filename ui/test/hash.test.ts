import assert from "node:assert/strict"
import { test } from "node:test"
import { hashOf } from "../src/logic/hash.ts"

test("a hash is 64 hex digits, in either case, with spaces around", () => {
  const hex = "00ff7a" + "11".repeat(29)
  const bytes = hashOf(`  ${hex.toUpperCase()}\n`)
  assert.deepEqual(bytes, Uint8Array.from(Buffer.from(hex, "hex")))
})

test("anything else is no hash", () => {
  for (const text of [
    "",
    "00".repeat(31),
    "00".repeat(33),
    "0x" + "00".repeat(31),
    "zz".repeat(32),
    "00 ".repeat(32),
  ]) {
    assert.equal(hashOf(text), undefined, text)
  }
})
