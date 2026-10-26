// An MCP server for one wallet, run by an AI client such as Claude Code:
//
//   claude mcp add b3wallet -e B3WALLET_ID=<wallet id> -- node <repo>/mcp/src/main.ts
//
// It calls the wallet with its own key (made on first run, see key.ts) as a
// proposer, so whatever the AI proposes waits for an owner. See README.md.
import { createClient } from "@ic-reactor/core"
import { readConfig } from "./config.ts"
import { loadOrCreateKey } from "./key.ts"
import { serve } from "./protocol.ts"
import { walletTools } from "./tools.ts"

const log = (line: string) => process.stderr.write(`b3wallet-mcp: ${line}\n`)

try {
  const config = readConfig(process.env)
  const { identity, created } = await loadOrCreateKey(config.keyPath)
  log(
    `${created ? "made a new key" : "key"} at ${config.keyPath}, principal ${identity.getPrincipal().toText()}`,
  )
  const client = createClient({ network: config.network, identity })
  await serve(
    walletTools(client, config),
    {
      name: "b3wallet",
      version: "0.1.0",
      instructions: [
        `You reach the wallet ${config.wallet} on ${config.network === "ic" ? "the Internet Computer" : "a local network"} as a proposer.`,
        "You read its accounts, balances, open actions and audit log, and propose transfers.",
        "You cannot approve, send or reject anything: every transfer you propose waits for an owner or an approver",
        `to approve and run it at ${config.pages}/approvals, so say so to the user each time.`,
        "Start with wallet_whoami. If you are not a member, call wallet_join and give the user the code it returns.",
        "The owners write what they want done as requests in the wallet: read them with wallet_requests and answer each with wallet_propose_transfer and its request id, so the wallet's guard checks your transfer against their own words.",
        `When the user asks you for a transfer in chat instead, ask them to write it as a request at ${config.pages}/approvals first.`,
      ].join(" "),
    },
    process.stdin,
    process.stdout,
    log,
  )
  client.dispose()
} catch (error) {
  log(error instanceof Error ? error.message : String(error))
  process.exitCode = 1
}
