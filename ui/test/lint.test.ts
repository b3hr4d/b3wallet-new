import assert from "node:assert/strict"
import { test } from "node:test"
import { ESLint } from "eslint"

const eslint = new ESLint({ cwd: new URL("..", import.meta.url).pathname })

/** The rules the lint reports for `code` written in `file` of the app. */
async function rulesFor(code: string, file = "src/injected.tsx"): Promise<string[]> {
  const [result] = await eslint.lintText(code, { filePath: file })
  return (result?.messages ?? []).map((message) => message.ruleId ?? message.message)
}

test("the lint refuses markup and code built from strings", async () => {
  for (const code of [
    `export const A = (p: { html: string }) => <div dangerouslySetInnerHTML={{ __html: p.html }} />`,
    `export const f = (e: Element, html: string) => { e.innerHTML = html }`,
    `export const f = (e: Element, html: string) => { e.outerHTML = html }`,
    `export const f = (e: Element, html: string) => e.insertAdjacentHTML("beforeend", html)`,
    `export const f = (html: string) => document.write(html)`,
    `export const A = () => <p style={{ color: "red" }}>x</p>`,
    `export const f = (code: string) => eval(code)`,
    `export const f = (code: string) => new Function(code)`,
    `export const f = () => setTimeout("go()", 0)`,
    `export const f = () => window.setInterval("go()", 9)`,
  ]) {
    const rules = await rulesFor(code)
    assert.ok(
      rules.some((rule) =>
        ["no-restricted-syntax", "no-eval", "no-new-func", "no-implied-eval"].includes(rule),
      ),
      `${code}: ${rules.join(", ")}`,
    )
  }
})

test("the lint passes plain elements and text", async () => {
  assert.deepEqual(
    await rulesFor(`export const A = (p: { text: string }) => <p className="quiet">{p.text}</p>\n`),
    [],
  )
})

test("the lint refuses links off the wallet's own origin", async () => {
  for (const code of [
    `export const A = () => <a href="https://example.com">x</a>`,
    `export const A = () => <a href="javascript:go()">x</a>`,
    `export const A = (p: { host: string }) => <a href={\`https://\${p.host}\`}>x</a>`,
    `export const go = () => "javascript:go()"`,
  ]) {
    assert.ok((await rulesFor(code)).includes("no-restricted-syntax"), code)
  }
  for (const code of [
    `export const A = () => <a href="/send">x</a>`,
    `export const A = () => <a href="#members">x</a>`,
    `export const A = (p: { id: number }) => <a href={\`/account?id=\${p.id}\`}>x</a>`,
  ]) {
    assert.deepEqual(await rulesFor(code), [], code)
  }
})

test("the lint keeps each layer to what it may import and reach", async () => {
  const refused: Array<[string, string]> = [
    ["src/pages/x.tsx", `import { h } from "preact"\nexport const a = h`],
    ["src/pages/x.tsx", `import { createPortal } from "react-dom"\nexport const a = createPortal`],
    [
      "src/pages/x.tsx",
      `import { useClient } from "@ic-reactor/react"\nexport const a = useClient`,
    ],
    [
      "src/parts/x.tsx",
      `import { HttpAgent } from "@icp-sdk/core/agent"\nexport const a = HttpAgent`,
    ],
    [
      "src/parts/x.tsx",
      `import { AuthClient } from "@icp-sdk/auth/client"\nexport const a = AuthClient`,
    ],
    [
      "src/main.tsx",
      `import { createClient } from "@ic-reactor/core"\nexport const a = createClient`,
    ],
    ["src/pages/x.tsx", `import { Home } from "./home.tsx"\nexport const a = Home`],
    ["src/parts/x.tsx", `import { Home } from "../pages/home.tsx"\nexport const a = Home`],
    ["src/ic/x.ts", `import { Reading } from "../parts/member.tsx"\nexport const a = Reading`],
    ["src/logic/x.ts", `import { useState } from "react"\nexport const a = useState`],
    ["src/logic/x.ts", `import { useWallet } from "../ic/canister.ts"\nexport const a = useWallet`],
    ["src/logic/x.ts", `export const a = () => window.location.hostname`],
    ["src/parts/x.tsx", `export const a = () => fetch("/x")`],
  ]
  for (const [file, code] of refused) {
    assert.ok((await rulesFor(code, file)).length > 0, `${file}: ${code}`)
  }
  const passed: Array<[string, string]> = [
    ["src/pages/x.tsx", `import { useAuth } from "@ic-reactor/react"\nexport const a = useAuth`],
    [
      "src/parts/x.tsx",
      `import { AgentError } from "@icp-sdk/core/agent"\nexport const a = AgentError`,
    ],
    ["src/ic/x.ts", `import { HttpAgent } from "@icp-sdk/core/agent"\nexport const a = HttpAgent`],
    ["src/live.tsx", `import { createPortal } from "react-dom"\nexport const a = createPortal`],
  ]
  for (const [file, code] of passed) {
    assert.deepEqual(await rulesFor(code, file), [], `${file}: ${code}`)
  }
})
