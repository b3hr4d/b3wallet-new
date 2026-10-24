import assert from "node:assert/strict"
import { test } from "node:test"
import { numberParam, where } from "../src/logic/host.ts"

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

test("a page reads a whole number from its address, and nothing else", () => {
  assert.equal(numberParam("?account=2", "account"), 2)
  assert.equal(numberParam("?id=0&s=owner", "id"), 0)
  assert.equal(numberParam("?account=4294967295", "account"), 4294967295)
  for (const search of [
    "",
    "?account=",
    "?account=-1",
    "?account=2.5",
    "?account=1e3",
    "?account=4294967296",
    "?account= 2",
  ]) {
    assert.equal(numberParam(search, "account"), undefined, search)
  }
})
