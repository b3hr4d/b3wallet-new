import assert from "node:assert/strict"
import { PassThrough } from "node:stream"
import { test } from "node:test"
import { checkArguments, InvalidArguments, serve, type Tool } from "../src/protocol.ts"

const echo: Tool = {
  name: "echo",
  title: "Echo",
  description: "Says what it is given.",
  inputSchema: {
    type: "object",
    properties: {
      word: { type: "string", description: "A word." },
      times: { type: "integer", description: "How often.", minimum: 1, maximum: 3 },
    },
    required: ["word"],
    additionalProperties: false,
  },
  readOnly: true,
  run: async (args) => {
    if (args.word === "boom") throw new Error("the wallet refused")
    return String(args.word).repeat(Number(args.times ?? 1))
  },
}

/** Sends `lines` to a server and returns what it answered, one message per line. */
async function exchange(lines: string[]): Promise<unknown[]> {
  const input = new PassThrough()
  const output = new PassThrough()
  const logged: string[] = []
  const done = serve(
    [echo],
    { name: "test", version: "1", instructions: "Be brief." },
    input,
    output,
    (line) => logged.push(line),
  )
  for (const line of lines) input.write(`${line}\n`)
  input.end()
  await done
  output.end()
  const text = output.read()?.toString() ?? ""
  return text
    .split("\n")
    .filter(Boolean)
    .map((line: string) => JSON.parse(line))
}

const request = (id: number, method: string, params?: object) =>
  JSON.stringify({ jsonrpc: "2.0", id, method, ...(params && { params }) })

test("it answers initialize, ping and tools/list, and not notifications", async () => {
  const replies = await exchange([
    request(1, "initialize", {
      protocolVersion: "2025-06-18",
      capabilities: {},
      clientInfo: { name: "claude-code", version: "2" },
    }),
    JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }),
    request(2, "ping"),
    request(3, "tools/list"),
  ])
  assert.deepEqual(replies, [
    {
      jsonrpc: "2.0",
      id: 1,
      result: {
        protocolVersion: "2025-06-18",
        capabilities: { tools: { listChanged: false } },
        serverInfo: { name: "test", version: "1" },
        instructions: "Be brief.",
      },
    },
    { jsonrpc: "2.0", id: 2, result: {} },
    {
      jsonrpc: "2.0",
      id: 3,
      result: {
        tools: [
          {
            name: "echo",
            title: "Echo",
            description: "Says what it is given.",
            inputSchema: echo.inputSchema,
            annotations: {
              title: "Echo",
              readOnlyHint: true,
              destructiveHint: false,
              idempotentHint: true,
              openWorldHint: true,
            },
          },
        ],
      },
    },
  ])
})

test("a version it does not speak gets its newest", async () => {
  const [reply] = await exchange([request(1, "initialize", { protocolVersion: "1999-01-01" })])
  assert.equal(
    (reply as { result: { protocolVersion: string } }).result.protocolVersion,
    "2025-11-25",
  )
})

test("a tool's result and failure are results; a missing tool or method is an error", async () => {
  const replies = await exchange([
    request(1, "tools/call", { name: "echo", arguments: { word: "ok", times: 2 } }),
    request(2, "tools/call", { name: "echo", arguments: { word: "boom" } }),
    request(3, "tools/call", { name: "echo", arguments: { word: "ok", times: 9 } }),
    request(4, "tools/call", { name: "approve", arguments: {} }),
    request(5, "resources/list"),
    "{not json",
    "[]",
  ])
  // Replies come as each request finishes; the protocol matches them by id.
  const order = (reply: unknown) => (reply as { id: number | null }).id ?? Infinity
  replies.sort((a, b) => order(a) - order(b))
  assert.deepEqual(replies, [
    {
      jsonrpc: "2.0",
      id: 1,
      result: { content: [{ type: "text", text: "okok" }], isError: false },
    },
    {
      jsonrpc: "2.0",
      id: 2,
      result: { content: [{ type: "text", text: "the wallet refused" }], isError: true },
    },
    {
      jsonrpc: "2.0",
      id: 3,
      result: { content: [{ type: "text", text: "times is from 1 to 3" }], isError: true },
    },
    { jsonrpc: "2.0", id: 4, error: { code: -32602, message: "Unknown tool: approve" } },
    { jsonrpc: "2.0", id: 5, error: { code: -32601, message: "Method not found: resources/list" } },
    { jsonrpc: "2.0", id: null, error: { code: -32700, message: "Parse error" } },
    {
      jsonrpc: "2.0",
      id: null,
      error: { code: -32600, message: "one JSON-RPC message per line, not a batch" },
    },
  ])
})

test("arguments are checked against the schema", () => {
  const schema = echo.inputSchema
  assert.deepEqual(checkArguments(schema, { word: "a" }), { word: "a" })
  for (const [args, reason] of [
    [undefined, "word is required"],
    [{}, "word is required"],
    [[], "the arguments are an object"],
    [{ word: 1 }, "word is a string"],
    [{ word: "a", times: 1.5 }, "times is an integer"],
    [{ word: "a", times: "2" }, "times is an integer"],
    [{ word: "a", times: 0 }, "times is from 1 to 3"],
    [{ word: "a", extra: true }, "there is no argument extra"],
  ] as const) {
    assert.throws(
      () => checkArguments(schema, args),
      (error: unknown) => error instanceof InvalidArguments && error.message === reason,
      JSON.stringify(args),
    )
  }
})
