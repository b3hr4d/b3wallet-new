import assert from "node:assert/strict"
import { test } from "node:test"
import { where } from "../src/host.ts"

const id = "efadq-gl777-77774-aaaba-cai"

test("a wallet's own address names it", () => {
  assert.deepEqual(where(`${id}.icp0.io`), { ok: true, wallet: id, local: false })
  assert.deepEqual(where(`${id.toUpperCase()}.ICP0.IO`), { ok: true, wallet: id, local: false })
  assert.deepEqual(where(`${id}.localhost`), { ok: true, wallet: id, local: true })
})

test("any other address is refused", () => {
  for (const host of [
    `${id}.raw.icp0.io`,
    `${id}.ic0.app`,
    `${id}.icp0.io.example.com`,
    `wallet.local.localhost`,
    `x.${id}.icp0.io`,
    "icp0.io",
    "localhost",
    "efadq-gl777-77774-aaaba-caj.icp0.io",
  ]) {
    assert.equal(where(host).ok, false, host)
  }
})
