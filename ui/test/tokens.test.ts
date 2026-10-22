import assert from "node:assert/strict"
import { test } from "node:test"
import type { MetadataValue } from "../src/generated/icrc1.ts"
import {
  amountParts,
  amountText,
  bitcoinToken,
  chainKey,
  chainText,
  cyclesText,
  ledgerOf,
  readAmount,
  tokenOf,
} from "../src/tokens.ts"

test("a typed amount is exactly its base units", () => {
  assert.deepEqual(readAmount("1.5", 8), { amount: 150_000_000n })
  assert.deepEqual(readAmount(" 0.29 ", 8), { amount: 29_000_000n })
  assert.deepEqual(readAmount("2", 0), { amount: 2n })
})

test("an amount that is not one to send says why", () => {
  assert.match(readAmount("1.123456789", 8).error ?? "", /at most 8 digits after the point/)
  assert.match(readAmount("1.5", 0).error ?? "", /no decimals/)
  for (const text of ["0", "0.0", "-1"]) {
    assert.equal(readAmount(text, 8).error, "Type an amount above zero.", text)
  }
  for (const text of ["1,5", "1e3", "abc", ".", "1 000"]) {
    assert.match(readAmount(text, 8).error ?? "", /Type an amount such as 1\.5/, text)
  }
})

test("a ledger's metadata gives its token", () => {
  const metadata: Array<[string, MetadataValue]> = [
    ["icrc1:name", { tag: "Text", value: "Internet Computer" }],
    ["icrc1:symbol", { tag: "Text", value: "ICP" }],
    ["icrc1:decimals", { tag: "Nat", value: 8n }],
    ["icrc1:fee", { tag: "Nat", value: 10_000n }],
  ]
  const token = tokenOf(metadata)
  assert.deepEqual(token, { symbol: "ICP", decimals: 8, fee: 10_000n })
  assert.equal(token && amountText(150_000_000n, token), "1.5 ICP")
  assert.deepEqual(tokenOf(metadata.filter(([key]) => key !== "icrc1:fee")), {
    symbol: "ICP",
    decimals: 8,
    fee: null,
  })
})

test("metadata without a symbol or usable decimals gives no token", () => {
  assert.equal(tokenOf([["icrc1:decimals", { tag: "Nat", value: 8n }]]), undefined)
  assert.equal(
    tokenOf([
      ["icrc1:symbol", { tag: "Text", value: "X" }],
      ["icrc1:decimals", { tag: "Nat", value: 256n }],
    ]),
    undefined,
  )
  assert.equal(
    tokenOf([
      ["icrc1:symbol", { tag: "Text", value: "X" }],
      ["icrc1:decimals", { tag: "Text", value: "8" }],
    ]),
    undefined,
  )
})

test("chains name their ledger and their token", () => {
  const ledger = "um5iw-rqaaa-aaaaq-qaaba-cai" as never
  assert.equal(ledgerOf({ tag: "Icp" }), "ryjl3-tyaaa-aaaaa-aaaba-cai")
  assert.equal(ledgerOf({ tag: "Icrc", value: ledger }), ledger)
  assert.equal(ledgerOf({ tag: "Bitcoin", value: { tag: "Mainnet" } }), undefined)
  assert.equal(chainText({ tag: "Bitcoin", value: { tag: "Regtest" } }), "Bitcoin regtest")
  assert.equal(chainText({ tag: "Icrc", value: ledger }), `ICRC-1 token of ledger ${ledger}`)
  assert.deepEqual(bitcoinToken({ tag: "Mainnet" }), { symbol: "BTC", decimals: 8, fee: null })
  assert.equal(bitcoinToken({ tag: "Testnet" }).symbol, "testnet BTC")
})

test("cycles read in trillions, rounded to a hundredth either way", () => {
  assert.equal(cyclesText(290_850_696_536n, "down"), "0.29T cycles")
  assert.equal(cyclesText(290_850_696_536n, "up"), "0.3T cycles")
  assert.equal(cyclesText(320_000_000_000n, "up"), "0.32T cycles")
  assert.equal(cyclesText(1_500_000_000_000n, "down"), "1.5T cycles")
  assert.equal(cyclesText(4_000_000_000n, "down"), "0T cycles")
})

test("a large amount groups its thousands and keeps every digit", () => {
  const icp = { symbol: "ICP", decimals: 8, fee: null }
  assert.deepEqual(amountParts(123_456_789_012n, icp), {
    whole: "1,234",
    fraction: ".56789012",
    symbol: "ICP",
  })
  assert.deepEqual(amountParts(100_000_000_000_000n, icp), {
    whole: "1,000,000",
    fraction: "",
    symbol: "ICP",
  })
  assert.deepEqual(amountParts(0n, icp), { whole: "0", fraction: "", symbol: "ICP" })
  assert.equal(amountParts(-123_400_000_000n, icp).whole, "-1,234")
  assert.equal(amountParts(999n, { symbol: "X", decimals: 0, fee: null }).whole, "999")
})

test("accounts of one chain share a key, and other chains have others", () => {
  const ledger = "mxzaz-hqaaa-aaaar-qaada-cai" as never
  assert.equal(chainKey({ tag: "Icp" }), chainKey({ tag: "Icp" }))
  const keys = [
    chainKey({ tag: "Icp" }),
    chainKey({ tag: "Icrc", value: ledger }),
    chainKey({ tag: "Icrc", value: "ryjl3-tyaaa-aaaaa-aaaba-cai" as never }),
    chainKey({ tag: "Bitcoin", value: { tag: "Mainnet" } }),
    chainKey({ tag: "Bitcoin", value: { tag: "Regtest" } }),
  ]
  assert.equal(new Set(keys).size, keys.length)
})
