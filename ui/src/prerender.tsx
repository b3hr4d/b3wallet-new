import { renderToString } from "react-dom/server"
import { App } from "./app.tsx"
import { PAGES } from "./page.ts"

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
      '<link rel="icon" href="data:,">',
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
