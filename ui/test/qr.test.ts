import assert from "node:assert/strict"
import { test } from "node:test"
import jsQR from "jsqr"
import { qrOf } from "../src/logic/qr.ts"

/**
 * What a camera would read: the QR code drawn at four pixels a module, dark
 * on light, decoded by jsQR, a decoder that shares no code with the encoder.
 */
function scan(text: string): string | undefined {
  const { size, modules } = qrOf(text)
  const scale = 4
  const width = size * scale
  const pixels = new Uint8ClampedArray(width * width * 4)
  for (let y = 0; y < width; y++) {
    for (let x = 0; x < width; x++) {
      const dark = modules[Math.floor(y / scale)]?.[Math.floor(x / scale)] === true
      pixels.fill(dark ? 0 : 255, (y * width + x) * 4, (y * width + x) * 4 + 3)
      pixels[(y * width + x) * 4 + 3] = 255
    }
  }
  return jsQR(pixels, width, width)?.data
}

test("an address's QR code reads back as exactly the address", () => {
  for (const address of [
    "uxrrr-q7777-77774-qaaaq-cai",
    "uxrrr-q7777-77774-qaaaq-cai-6cc627i.1",
    "fvski-cdv2h-g6ldz-eacds-4wfcf-px5sn-yfwv7-bwtxb-wvytu-cm3qa-sae-dfxgiyy.102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20",
    "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
    "bc1p5d7rjq7g6rdk2yhzks9smlaqtedr4dekq08ge8ztwac72sfr9rusxg3297",
    "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080",
    // An ICP account identifier, the form exchanges ask for.
    "8af54f1fa09faeca18d294e0787346264f9f1d6189ed20ff14f029a160b787e8",
  ]) {
    assert.equal(scan(address), address, address)
  }
})

test("the path draws exactly the dark modules", () => {
  const { size, path, modules } = qrOf("uxrrr-q7777-77774-qaaaq-cai")
  const drawn = modules.map((row) => row.map(() => false))
  for (const [, x, y, run] of path.matchAll(/M(\d+) (\d+)h(\d+)v1h-\d+z/g)) {
    for (let i = 0; i < Number(run); i++) drawn[Number(y)]![Number(x) + i] = true
  }
  assert.deepEqual(drawn, modules)
  // The quiet zone: four light modules on every side.
  for (let i = 0; i < size; i++) {
    for (const edge of [0, 1, 2, 3, size - 4, size - 3, size - 2, size - 1]) {
      assert.equal(modules[edge]?.[i], false)
      assert.equal(modules[i]?.[edge], false)
    }
  }
})
