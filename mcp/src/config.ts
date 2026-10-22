// What the server reads from its environment: which wallet, on which
// network, and where the agent's key is. `claude mcp add` sets them with -e.
import { homedir } from "node:os"
import { join } from "node:path"
import { Principal } from "@icp-sdk/core/principal"

export type Config = {
  /** The wallet canister's id. */
  wallet: string
  network: "ic" | "local"
  /** The wallet's own pages, where an owner approves what the agent proposes. */
  pages: string
  /** The agent's key file. */
  keyPath: string
}

/** The local network's port, as icp-cli and scripts/try-local.sh run it. */
const LOCAL_PORT = 8000

export function readConfig(env: Record<string, string | undefined>): Config {
  const wallet = env.B3WALLET_ID?.trim()
  if (!wallet) throw new Error("set B3WALLET_ID to the wallet's canister id")
  let id: Principal
  try {
    id = Principal.fromText(wallet)
  } catch {
    throw new Error(`B3WALLET_ID is not a canister id: ${wallet}`)
  }
  const network = env.B3WALLET_NETWORK?.trim() || "ic"
  if (network !== "ic" && network !== "local") {
    throw new Error(`B3WALLET_NETWORK is "ic" or "local", not ${network}`)
  }
  const text = id.toText()
  return {
    wallet: text,
    network,
    pages: network === "ic" ? `https://${text}.icp0.io` : `http://${text}.localhost:${LOCAL_PORT}`,
    keyPath: env.B3WALLET_KEY?.trim() || join(homedir(), ".config", "b3wallet-mcp", "agent.pem"),
  }
}
