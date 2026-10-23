import { encode } from "uqr"

/** A QR code to draw: its width in modules, and its dark modules as one path. */
export type Qr = { size: number; path: string; modules: boolean[][] }

/**
 * The QR code of `text`, exactly as given: medium error correction, which
 * survives a smudge or a glare, and the four-module quiet zone scanners need.
 * The path draws each row's runs of dark modules, so a QR code of an address
 * is one element, drawn in the page with no style.
 */
export function qrOf(text: string): Qr {
  const { size, data } = encode(text, { ecc: "M", border: 4 })
  let path = ""
  data.forEach((row, y) => {
    let x = 0
    while (x < size) {
      if (!row[x]) {
        x++
        continue
      }
      const start = x
      while (x < size && row[x]) x++
      path += `M${start} ${y}h${x - start}v1h${start - x}z`
    }
  })
  return { size, path, modules: data }
}
