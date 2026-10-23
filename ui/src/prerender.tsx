import { renderToString } from "react-dom/server"
import { App } from "./app.tsx"
import { THREE } from "./brand.tsx"
import { PAGES } from "./page.ts"

/**
 * B3Wallet's favicon, `b3wallet-favicon.svg` of B3Pay's design system: the
 * mark on its graphite tile. It is inline, since the CSP lets images come from
 * data URLs and an inline icon is no extra file in the release.
 */
const ICON = `data:image/svg+xml,${encodeURIComponent(
  [
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 96 96">',
    '<rect width="96" height="96" rx="22" fill="#121110"/>',
    '<g transform="translate(14 14) scale(.7)">',
    '<rect x="18.75" y="11" width="14" height="74" rx="3" fill="#f1ede6"/>',
    `<path d="${THREE}" fill="#f1ede6"/>`,
    '<circle cx="54.75" cy="62.5" r="6.5" fill="#f0b35a"/>',
    "</g></svg>",
  ].join(""),
)}`

/**
 * Every page as the file the wallet serves: the same for every visitor, with
 * the app's script and styles from the wallet's own origin and nothing
 * inline, as the CSP requires.
 */
export function prerender(): Array<{ file: string; html: string }> {
  return PAGES.map(({ page, file, title }) => ({
    file,
    html: [
      "<!doctype html>",
      '<html lang="en">',
      "<head>",
      '<meta charset="utf-8">',
      '<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">',
      `<title>${title}</title>`,
      '<meta name="color-scheme" content="dark light">',
      '<meta name="theme-color" content="#100f0d" media="(prefers-color-scheme: dark)">',
      '<meta name="theme-color" content="#f1eee8" media="(prefers-color-scheme: light)">',
      `<link rel="icon" href="${ICON}">`,
      '<link rel="stylesheet" href="/app.css">',
      '<script type="module" src="/app.js"></script>',
      "</head>",
      "<body>",
      `<div id="root" data-page="${page}">${renderToString(<App page={page} />)}</div>`,
      "</body>",
      "</html>",
      "",
    ].join("\n"),
  }))
}
