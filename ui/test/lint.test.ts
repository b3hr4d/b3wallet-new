import assert from "node:assert/strict"
import { test } from "node:test"
import { ESLint } from "eslint"

const eslint = new ESLint({ cwd: new URL("..", import.meta.url).pathname })

/** The rules the lint reports for `code` written in a file of the app. */
async function rulesFor(code: string): Promise<string[]> {
  const [result] = await eslint.lintText(code, { filePath: "src/injected.tsx" })
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
