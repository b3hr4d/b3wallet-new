/**
 * A wallet's mark: a hue and a pattern of dots drawn from its canister id, so
 * that someone with several wallets tells them apart at a glance. It proves
 * nothing about which wallet a page belongs to, since a page at another
 * address can draw any mark; the address does that.
 */

/** How many hues app.css has a class for, `hue-0` to `hue-23`, 15 degrees apart. */
export const HUES = 24

/** FNV-1a over the text's UTF-16 code units: the same 32 bits in every browser. */
export function hashOf(text: string): number {
  let hash = 0x811c9dc5
  for (let i = 0; i < text.length; i++) {
    hash ^= text.charCodeAt(i)
    hash = Math.imul(hash, 0x01000193)
  }
  return hash >>> 0
}

/** The class that gives an element the hue of `text`. */
export function hueClass(text: string): string {
  return `hue-${hashOf(text) % HUES}`
}

/**
 * The dots of a 5 by 5 grid that `text` turns on, as `[column, row]`,
 * mirrored left to right. Fifteen bits pick the left three columns; a pattern
 * with fewer than four dots takes the other fifteen instead.
 */
export function dotsOf(text: string): Array<[number, number]> {
  const hash = hashOf(`${text}/dots`)
  const pick = (bits: number) => {
    const dots: Array<[number, number]> = []
    for (let row = 0; row < 5; row++) {
      for (let column = 0; column < 3; column++) {
        if ((bits >>> (row * 3 + column)) & 1) {
          dots.push([column, row])
          if (column < 2) dots.push([4 - column, row])
        }
      }
    }
    return dots
  }
  const dots = pick(hash)
  return dots.length >= 4 ? dots : pick(~hash & 0x7fff)
}

/** The first letter of a name, for an avatar: `L` for “laptop”. */
export function initialOf(name: string): string {
  const [first] = name.trim()
  return first === undefined ? "?" : first.toUpperCase()
}
