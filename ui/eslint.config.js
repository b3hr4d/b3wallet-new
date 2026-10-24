// The rules that keep injected markup and code out of the wallet's pages, and
// each part of the app in its layer, on top of the recommended ones. The CSP
// blocks inline script and style too; these catch them before a release is
// built. docs/ui-standard.md says why each rule is there.
import js from "@eslint/js"
import { defineConfig } from "eslint/config"
import reactHooks from "eslint-plugin-react-hooks"
import tseslint from "typescript-eslint"

const markup = "React's escaping stays on: render text and elements, never HTML strings"
const link = "Link only to the wallet's own pages: a path from / or a #fragment"
const urlAttribute = "JSXAttribute[name.name=/^(href|src|action|formAction)$/]"

// What every module may not import. The pages are written against React and
// ship on Preact (decision 0011), so no module names Preact; only the entry
// points render; and only ic/ makes clients, agents and actors or signs in,
// so every call goes to a host the CSP allows and every read is certified.
const preact = {
  group: ["preact", "preact/*"],
  message: "Write against React's API: the build bundles Preact in its place (decision 0011)",
}
const renderer = {
  group: ["react-dom", "react-dom/*"],
  message: "Only main.tsx, prerender.tsx and live.tsx render",
}
const icOnly = "Only ic/ reaches the IC: use useWallet, useToken or walletClient"
const makers = [
  { name: "@icp-sdk/auth", message: icOnly },
  { name: "@icp-sdk/auth/client", message: icOnly },
  { name: "@icp-sdk/core/agent", importNames: ["HttpAgent", "Actor"], message: icOnly },
  { name: "@ic-reactor/core", importNames: ["createClient"], message: icOnly },
  { name: "@ic-reactor/react", importNames: ["useClient", "ReactorProvider"], message: icOnly },
]
const root = {
  regex: "^\\.\\./[^/]+\\.tsx?$",
  message: "Only the entry points import app.tsx and live.tsx",
}

/** The imports `layer` may not make, with what no module may. */
function imports(layer) {
  return [
    "error",
    {
      paths: layer.paths ?? makers,
      patterns: [preact, ...(layer.renders ? [] : [renderer]), ...(layer.patterns ?? [])],
    },
  ]
}

// The network is the IC, through ic/: nothing else fetches, opens a socket or
// sends a beacon.
const network = ["fetch", "XMLHttpRequest", "WebSocket", "EventSource"].map((name) => ({
  name,
  message: icOnly,
}))
// logic/ runs the same in Node, in the prerender and in the tests as in the
// browser: it reads nothing of the page, the browser or the network.
const browser = ["window", "document", "location", "navigator", "localStorage", "sessionStorage"]
  .map((name) => ({ name, message: "logic/ is plain TypeScript: take what it needs as arguments" }))
  .concat(network)

export default defineConfig(
  { ignores: ["node_modules/", "src/generated/", ".build/"] },
  js.configs.recommended,
  tseslint.configs.recommended,
  {
    files: ["src/**/*.{ts,tsx}"],
    extends: [reactHooks.configs.flat.recommended],
    rules: { "react-hooks/exhaustive-deps": "error" },
  },
  {
    files: ["src/**/*.{ts,tsx}", "test/**/*.ts"],
    // Declared so no-implied-eval sees a string handed to them.
    languageOptions: {
      globals: { setTimeout: "readonly", setInterval: "readonly", window: "readonly" },
    },
    rules: {
      "no-eval": "error",
      "no-implied-eval": "error",
      "no-new-func": "error",
      "no-restricted-syntax": [
        "error",
        { selector: "JSXAttribute[name.name='dangerouslySetInnerHTML']", message: markup },
        { selector: "MemberExpression[property.name=/^(innerHTML|outerHTML)$/]", message: markup },
        { selector: "CallExpression[callee.property.name='insertAdjacentHTML']", message: markup },
        {
          selector:
            "CallExpression[callee.object.name='document'][callee.property.name=/^write(ln)?$/]",
          message: markup,
        },
        {
          selector: "JSXAttribute[name.name='style']",
          message: "The CSP allows no inline style: give the element a class from app.css",
        },
        // React 19 refuses a javascript: URL in a link; Preact does not, so
        // the pages never write one, and link only to their own origin.
        { selector: "Literal[value=/^\\s*javascript:/i]", message: link },
        { selector: "TemplateElement[value.raw=/^\\s*javascript:/i]", message: link },
        { selector: `${urlAttribute} > Literal[value=/^(?![/#])/]`, message: link },
        {
          selector: `${urlAttribute} > JSXExpressionContainer > Literal[value=/^(?![/#])/]`,
          message: link,
        },
        {
          selector: `${urlAttribute} > JSXExpressionContainer > TemplateLiteral > TemplateElement:first-child[value.raw=/^(?![/#])/]`,
          message: link,
        },
      ],
    },
  },
  {
    files: ["src/**/*.{ts,tsx}"],
    rules: {
      "@typescript-eslint/no-restricted-imports": imports({}),
      "no-restricted-globals": ["error", ...network],
      "no-restricted-properties": [
        "error",
        { object: "navigator", property: "sendBeacon", message: icOnly },
      ],
    },
  },
  // The entry points: they render, and live.tsx gives the pages their client.
  {
    files: ["src/main.tsx", "src/prerender.tsx", "src/live.tsx"],
    rules: {
      "@typescript-eslint/no-restricted-imports": imports({
        renders: true,
        paths: makers.map((path) =>
          path.name === "@ic-reactor/react" ? { ...path, importNames: ["useClient"] } : path,
        ),
      }),
    },
  },
  // ic/: the only code that reaches the IC. It sits under the pages and their
  // parts, and imports neither.
  {
    files: ["src/ic/**"],
    rules: {
      "@typescript-eslint/no-restricted-imports": imports({
        paths: [],
        patterns: [
          { regex: "^\\.\\./(parts|pages)/", message: "ic/ sits under the parts and pages" },
          root,
        ],
      }),
      "no-restricted-globals": "off",
      "no-restricted-properties": "off",
    },
  },
  // pages/: one module per page. A page shares through parts/, never by
  // importing another page.
  {
    files: ["src/pages/**"],
    rules: {
      "@typescript-eslint/no-restricted-imports": imports({
        patterns: [
          { regex: "^\\./", message: "A page imports no other page: share it from parts/" },
          root,
        ],
      }),
    },
  },
  // parts/: what pages share. It imports no page.
  {
    files: ["src/parts/**"],
    rules: {
      "@typescript-eslint/no-restricted-imports": imports({
        patterns: [{ regex: "^\\.\\./pages/", message: "parts/ imports no page" }, root],
      }),
    },
  },
  // logic/: plain TypeScript, tested in Node. No React, no browser, no network.
  {
    files: ["src/logic/**"],
    rules: {
      "@typescript-eslint/no-restricted-imports": imports({
        patterns: [
          {
            group: ["react", "react/*", "@tanstack/*", "@ic-reactor/react", "@icp-sdk/auth*"],
            message: "logic/ is plain TypeScript: no React and no client",
          },
          {
            regex: "^\\.\\./(ic|parts|pages)/",
            message: "logic/ sits under everything else and imports none of it",
          },
          root,
        ],
      }),
      "no-restricted-globals": ["error", ...browser],
    },
  },
  {
    files: ["*.js", "*.mjs"],
    languageOptions: {
      globals: { console: "readonly", process: "readonly", URL: "readonly" },
    },
  },
)
