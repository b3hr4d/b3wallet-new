import { renderToString } from "react-dom/server"
import { App } from "./app.tsx"
import { PAGES } from "./page.ts"

/**
 * The logo as the tab's icon, inline: the CSP lets images come from data URLs,
 * and an inline icon is no extra file in the release.
 */
const ICON = `data:image/svg+xml,${encodeURIComponent(
  [
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">',
    '<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1">',
    '<stop offset="0" stop-color="#8a7dff"/><stop offset="1" stop-color="#4d39e0"/>',
    "</linearGradient></defs>",
    '<rect width="32" height="32" rx="9" fill="url(#g)"/>',
    '<circle cx="16" cy="13.4" r="4.4" fill="#fff"/>',
    '<path d="M13.5 16.3h5l1.1 7.2h-7.2z" fill="#fff"/>',
    "</svg>",
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
      '<meta name="viewport" content="width=device-width, initial-scale=1">',
      `<title>${title}</title>`,
      '<meta name="color-scheme" content="light dark">',
      '<meta name="theme-color" content="#f4f4f1" media="(prefers-color-scheme: light)">',
      '<meta name="theme-color" content="#0a0b0d" media="(prefers-color-scheme: dark)">',
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
