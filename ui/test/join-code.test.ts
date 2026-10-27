import assert from "node:assert/strict"
import { createHash } from "node:crypto"
import { test } from "node:test"
import { joinCode } from "../src/logic/join-code.ts"

const device = "gxhcg-wz6ja-zbwh7-s6ha4-nnlhc-ya5hh-2dv23-dwksw-ujozj-ymkcv-tae"

test("a join code is eight digits from the SHA-256 of the principal's text", () => {
  const hash = createHash("sha256").update(device).digest()
  const digits = String(hash.readUInt32BE(0) % 100_000_000).padStart(8, "0")
  assert.equal(joinCode(device), `${digits.slice(0, 4)} ${digits.slice(4)}`)
  assert.match(joinCode(device), /^\d{4} \d{4}$/)
})

test("another principal shows another code", () => {
  const codes = new Set(
    ["2vxsx-fae", "aaaaa-aa", "efadq-gl777-77774-aaaba-cai", device].map(joinCode),
  )
  assert.equal(codes.size, 4)
})
