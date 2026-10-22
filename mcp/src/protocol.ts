// The Model Context Protocol over stdio, for a server that offers tools and
// nothing else: JSON-RPC 2.0 messages, one per line, in on stdin and out on
// stdout. Logs go to stderr, which the client shows but never parses.
//
// It is written here rather than taken from the SDK so that the process that
// holds the agent's key runs only this repository's code and the IC
// libraries the pages already use.
import { createInterface } from "node:readline"
import type { Readable, Writable } from "node:stream"

/** Protocol versions this server speaks, newest first. */
export const PROTOCOL_VERSIONS = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"]

/** A JSON Schema for a tool's arguments: an object of strings and integers. */
export type InputSchema = {
  type: "object"
  properties: Record<
    string,
    | { type: "string"; description: string }
    | { type: "integer"; description: string; minimum?: number; maximum?: number }
  >
  required?: string[]
  additionalProperties: false
}

export type Tool = {
  name: string
  title: string
  description: string
  inputSchema: InputSchema
  /** Whether the tool only reads. A tool that proposes is not read-only. */
  readOnly: boolean
  /** Text for the model, or an error whose message the model reads. */
  run(args: Record<string, unknown>): Promise<string>
}

export type ServerInfo = { name: string; version: string; instructions: string }

/** Thrown for arguments the schema refuses: the model reads why. */
export class InvalidArguments extends Error {}

type Message = { jsonrpc?: unknown; id?: unknown; method?: unknown; params?: unknown }

const PARSE_ERROR = -32700
const INVALID_REQUEST = -32600
const METHOD_NOT_FOUND = -32601
const INVALID_PARAMS = -32602

/** Checks `args` against a tool's schema, and returns them. */
export function checkArguments(schema: InputSchema, args: unknown): Record<string, unknown> {
  if (args === undefined) args = {}
  if (typeof args !== "object" || args === null || Array.isArray(args)) {
    throw new InvalidArguments("the arguments are an object")
  }
  const given = args as Record<string, unknown>
  for (const name of Object.keys(given)) {
    if (!Object.hasOwn(schema.properties, name)) {
      throw new InvalidArguments(`there is no argument ${name}`)
    }
  }
  for (const name of schema.required ?? []) {
    if (given[name] === undefined) throw new InvalidArguments(`${name} is required`)
  }
  for (const [name, property] of Object.entries(schema.properties)) {
    const value = given[name]
    if (value === undefined) continue
    if (property.type === "string" && typeof value !== "string") {
      throw new InvalidArguments(`${name} is a string`)
    }
    if (property.type === "integer") {
      if (typeof value !== "number" || !Number.isSafeInteger(value)) {
        throw new InvalidArguments(`${name} is an integer`)
      }
      if (
        (property.minimum !== undefined && value < property.minimum) ||
        (property.maximum !== undefined && value > property.maximum)
      ) {
        throw new InvalidArguments(
          `${name} is from ${property.minimum ?? "any"} to ${property.maximum ?? "any"}`,
        )
      }
    }
  }
  return given
}

/**
 * Answers MCP requests from `input` on `output` until `input` ends. A tool's
 * failure is a result the model reads (`isError`), not a protocol error.
 */
export function serve(
  tools: readonly Tool[],
  info: ServerInfo,
  input: Readable,
  output: Writable,
  log: (line: string) => void,
): Promise<void> {
  const byName = new Map(tools.map((tool) => [tool.name, tool]))
  const send = (message: object) => output.write(`${JSON.stringify(message)}\n`)
  const reply = (id: unknown, result: object) => send({ jsonrpc: "2.0", id, result })
  const fail = (id: unknown, code: number, message: string) =>
    send({ jsonrpc: "2.0", id, error: { code, message } })

  const handle = async (message: Message): Promise<void> => {
    const { id, method, params } = message
    const request = id !== undefined
    if (message.jsonrpc !== "2.0" || typeof method !== "string") {
      if (request) fail(id, INVALID_REQUEST, "not a JSON-RPC 2.0 request")
      return
    }
    // Notifications (initialized, cancelled, roots changed) need no answer.
    if (!request) return
    const p = (typeof params === "object" && params !== null ? params : {}) as Record<
      string,
      unknown
    >
    switch (method) {
      case "initialize": {
        const asked = p.protocolVersion
        const version =
          typeof asked === "string" && PROTOCOL_VERSIONS.includes(asked)
            ? asked
            : PROTOCOL_VERSIONS[0]
        reply(id, {
          protocolVersion: version,
          capabilities: { tools: { listChanged: false } },
          serverInfo: { name: info.name, version: info.version },
          instructions: info.instructions,
        })
        return
      }
      case "ping":
        reply(id, {})
        return
      case "tools/list":
        reply(id, {
          tools: tools.map((tool) => ({
            name: tool.name,
            title: tool.title,
            description: tool.description,
            inputSchema: tool.inputSchema,
            annotations: {
              title: tool.title,
              readOnlyHint: tool.readOnly,
              destructiveHint: false,
              idempotentHint: tool.readOnly,
              openWorldHint: true,
            },
          })),
        })
        return
      case "tools/call": {
        const tool = typeof p.name === "string" ? byName.get(p.name) : undefined
        if (tool === undefined) {
          fail(id, INVALID_PARAMS, `Unknown tool: ${String(p.name)}`)
          return
        }
        let text: string
        let isError = false
        try {
          text = await tool.run(checkArguments(tool.inputSchema, p.arguments))
        } catch (error) {
          isError = true
          text = error instanceof Error ? error.message : String(error)
          if (!(error instanceof InvalidArguments)) log(`${tool.name}: ${text}`)
        }
        reply(id, { content: [{ type: "text", text }], isError })
        return
      }
      default:
        fail(id, METHOD_NOT_FOUND, `Method not found: ${method}`)
    }
  }

  return new Promise((resolve) => {
    const lines = createInterface({ input, crlfDelay: Infinity })
    const running = new Set<Promise<void>>()
    lines.on("line", (line) => {
      if (!line.trim()) return
      let message: unknown
      try {
        message = JSON.parse(line)
      } catch {
        fail(null, PARSE_ERROR, "Parse error")
        return
      }
      if (typeof message !== "object" || message === null || Array.isArray(message)) {
        fail(null, INVALID_REQUEST, "one JSON-RPC message per line, not a batch")
        return
      }
      const done = handle(message as Message)
        .catch((error: unknown) => log(`unexpected: ${String(error)}`))
        .finally(() => running.delete(done))
      running.add(done)
    })
    lines.on("close", () => {
      void Promise.allSettled(running).then(() => resolve())
    })
  })
}
