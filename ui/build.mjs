// Builds the wallet's pages into canisters/wallet/pages, which the wallet
// embeds in its Wasm: app.js and app.css for the browser, and one prerendered
// HTML file per page. The same sources, lockfile, Node and esbuild give the
// same bytes; CI builds twice in two directories and compares them.
//
//   node build.mjs [--out <dir>]
import { Buffer } from "node:buffer"
import { execFileSync } from "node:child_process"
import { mkdir, readFile, rm, writeFile } from "node:fs/promises"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"
import * as esbuild from "esbuild"

const ui = dirname(fileURLToPath(import.meta.url))
const args = process.argv.slice(2)
const out = resolve(
  args[0] === "--out" && args[1] ? args[1] : join(ui, "../canisters/wallet/pages"),
)

const { engines } = JSON.parse(await readFile(join(ui, "package.json"), "utf8"))
if (process.version !== `v${engines.node}`) {
  throw new Error(`the build is pinned to Node ${engines.node}, and this is ${process.version}`)
}

// The bindings come from the wallet's interface on every build, so the app
// cannot call the wallet with an interface it no longer has; the ledger's
// are of the part of ICRC-1 the app reads.
execFileSync(
  process.execPath,
  [
    join(ui, "node_modules/@candid-core/cli/bin/cli.js"),
    "gen",
    join(ui, "../candid/wallet.did"),
    join(ui, "candid/icrc1.did"),
    "-o",
    join(ui, "src/generated"),
  ],
  { stdio: ["ignore", "ignore", "inherit"] },
)

const shared = {
  absWorkingDir: ui,
  bundle: true,
  charset: "utf8",
  define: { "process.env.NODE_ENV": '"production"' },
  legalComments: "eof",
  logLevel: "warning",
}

// The browser's bundle, written to memory first so a failed build leaves the
// pages as they were.
const browser = await esbuild.build({
  ...shared,
  entryPoints: [
    { in: "src/main.tsx", out: "app" },
    { in: "src/app.css", out: "app" },
  ],
  format: "esm",
  minify: true,
  outdir: out,
  platform: "browser",
  target: "es2022",
  write: false,
})

// The same App, rendered in Node to the HTML each page starts from.
const scratch = join(ui, ".build")
await rm(scratch, { recursive: true, force: true })
await esbuild.build({
  ...shared,
  entryPoints: ["src/prerender.tsx"],
  format: "esm",
  outfile: join(scratch, "prerender.mjs"),
  packages: "external",
  platform: "node",
  target: "node22",
})
const { prerender } = await import(pathToFileURL(join(scratch, "prerender.mjs")).href)
const pages = prerender()
await rm(scratch, { recursive: true, force: true })

const files = [
  ...browser.outputFiles.map((file) => ({
    file: file.path.slice(out.length + 1),
    bytes: file.contents,
  })),
  ...pages.map(({ file, html }) => ({ file, bytes: Buffer.from(html, "utf8") })),
]
const expected = [
  "404.html",
  "account.html",
  "app.css",
  "app.js",
  "approvals.html",
  "claim.html",
  "index.html",
  "members.html",
  "receive.html",
  "send.html",
  "settings.html",
]
const names = files.map(({ file }) => file).sort()
if (names.join() !== expected.join()) {
  throw new Error(`the build made ${names.join(", ")}, not ${expected.join(", ")}`)
}

await mkdir(out, { recursive: true })
for (const { file, bytes } of files) {
  await writeFile(join(out, file), bytes)
  console.log(`${file.padEnd(12)} ${String(bytes.length).padStart(8)} bytes`)
}
