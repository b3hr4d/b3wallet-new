import assert from "node:assert/strict"
import { test } from "node:test"
import type { MetadataValue } from "../src/generated/icrc1.ts"
import type { Account, Action, Chain } from "../src/generated/wallet.ts"
import {
  amountParts,
  amountText,
  assetsOf,
  bitcoinToken,
  chainKey,
  chainAmountText,
  chainText,
  cyclesText,
  defaultAsset,
  evmChainsOf,
  evmToken,
  kindText,
  ledgerOf,
  readAmount,
  tokenOf,
  transferAsset,
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
  assert.equal(ledgerOf({ tag: "Evm", value: 1n }), undefined)
  assert.equal(chainText({ tag: "Evm", value: 8_453n }), "Base")
  assert.equal(chainText({ tag: "Evm", value: 56n }), "EVM chain 56")
  assert.deepEqual(evmToken(1n), { symbol: "ETH", decimals: 18, fee: null })
  assert.equal(evmToken(8_453n).symbol, "ETH on Base")
  assert.equal(chainAmountText(1_500_000_000_000_000_000n, { tag: "Evm", value: 1n }), "1.5 ETH")
})

test("a wallet on a test network holds Sepolia alone", () => {
  const ids = (network: "Mainnet" | "Testnet" | "Regtest") =>
    evmChainsOf({ tag: network }).map((chain) => chain.id)
  assert.deepEqual(ids("Mainnet"), [1n, 11_155_111n, 42_161n, 8_453n, 10n])
  assert.deepEqual(ids("Testnet"), [11_155_111n])
  assert.deepEqual(ids("Regtest"), [11_155_111n])
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

test("an account shows the assets of its kind, its default first", () => {
  const ledger = "mxzaz-hqaaa-aaaar-qaada-cai" as never
  const token: Chain = { tag: "Icrc", value: ledger }
  const base: Chain = { tag: "Evm", value: 8_453n }
  const tracked: Chain[] = [{ tag: "Icp" }, { tag: "Evm", value: 1n }, token, base]
  const ic: Account = {
    id: 1,
    kind: { tag: "InternetComputer" },
    label: "Main",
    address: "efadq-gl777-77774-aaaba-cai",
    icp_account_id: null,
    index: 0,
    holder: { tag: "Wallet" },
  }
  const btc: Account = { ...ic, id: 2, kind: { tag: "Bitcoin", value: { tag: "Regtest" } } }
  const evm: Account = { ...ic, id: 3, kind: { tag: "Evm" } }
  assert.deepEqual(assetsOf(ic, tracked), [{ tag: "Icp" }, token])
  assert.deepEqual(assetsOf(btc, tracked), [{ tag: "Bitcoin", value: { tag: "Regtest" } }])
  assert.deepEqual(assetsOf(evm, tracked), [{ tag: "Evm", value: 1n }, base])
  assert.equal(defaultAsset(evm.kind), undefined)
  assert.deepEqual(defaultAsset(ic.kind), { tag: "Icp" })
  assert.deepEqual(defaultAsset(btc.kind), { tag: "Bitcoin", value: { tag: "Regtest" } })
  assert.equal(kindText(ic.kind), "Internet Computer")
  assert.equal(kindText(btc.kind), "Bitcoin regtest")

  // A transfer sends the asset it names, or its account's default.
  const send = (asset: Chain | null): Action => ({
    tag: "Transfer",
    value: { from: 1, asset, to: "x", amount: 1n, fee: null, memo: null },
  })
  assert.deepEqual(transferAsset(send(token), ic), token)
  assert.deepEqual(transferAsset(send(null), ic), { tag: "Icp" })
  assert.deepEqual(transferAsset(send(null), btc), { tag: "Bitcoin", value: { tag: "Regtest" } })
  assert.deepEqual(transferAsset(send(base), evm), base)
  assert.equal(transferAsset(send(null), undefined), undefined)
  assert.equal(transferAsset({ tag: "RemoveMember", value: ledger }, ic), undefined)
})
