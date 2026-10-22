import assert from "node:assert/strict"
import { test } from "node:test"
import { dotsOf, hashOf, HUES, hueClass, initialOf } from "../src/mark.ts"

const wallet = "uxrrr-q7777-77774-qaaaq-cai"
const other = "u6s2n-gx777-77774-qaaba-cai"

test("a wallet's mark is the same on every visit", () => {
  assert.equal(hashOf(""), 0x811c9dc5)
  assert.equal(hashOf("a"), 0xe40c292c)
  assert.equal(hueClass(wallet), hueClass(wallet))
  assert.deepEqual(dotsOf(wallet), dotsOf(wallet))
  assert.notDeepEqual(dotsOf(wallet), dotsOf(other))
})

test("a mark's hue is one app.css has a class for", () => {
  for (const id of [wallet, other, "aaaaa-aa", "2vxsx-fae"]) {
    const hue = Number(/^hue-(\d+)$/.exec(hueClass(id))?.[1])
    assert.ok(Number.isInteger(hue) && hue >= 0 && hue < HUES, id)
  }
})

test("a mark's dots fill a mirrored 5 by 5 grid, four at least", () => {
  for (const id of [wallet, other, "aaaaa-aa", "2vxsx-fae", "ryjl3-tyaaa-aaaaa-aaaba-cai"]) {
    const dots = dotsOf(id)
    assert.ok(dots.length >= 4, id)
    const on = new Set(dots.map(([column, row]) => `${column},${row}`))
    assert.equal(on.size, dots.length, id)
    for (const [column, row] of dots) {
      assert.ok(column >= 0 && column < 5 && row >= 0 && row < 5, id)
      assert.ok(on.has(`${4 - column},${row}`), id)
    }
  }
})

test("an avatar shows a name's first letter", () => {
  assert.equal(initialOf("laptop"), "L")
  assert.equal(initialOf("  phone"), "P")
  assert.equal(initialOf("🦊 fox"), "🦊")
  assert.equal(initialOf(""), "?")
})
