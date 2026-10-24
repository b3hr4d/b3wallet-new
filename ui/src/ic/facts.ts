import { CanisterStatus, HttpAgent } from "@icp-sdk/core/agent"
import { Principal } from "@icp-sdk/core/principal"

/** What the IC itself says of the wallet's canister: who controls it, and the hash of its module. */
export type Facts = { controllers: string[]; moduleHash: string | undefined }

/**
 * Reads the wallet's controllers and module hash from the IC's certified
 * state, as anyone may. The agent checks the certificate against the IC's
 * root key, which mainnet's agent holds in its code; only on icp-cli's local
 * network does it fetch the key from the replica, as local development must.
 * The page itself vouches for none of it: a page that lies could show any
 * answer, so the command line's `icp canister status` stays the proof.
 *
 * `CanisterStatus` is deprecated in favour of `readState` with state paths;
 * it still reads and verifies both paths in `@icp-sdk/core` 6.1.
 */
export async function readFacts(wallet: string, local: boolean): Promise<Facts> {
  const agent = await HttpAgent.create({
    host: local ? window.location.origin : "https://icp-api.io",
    shouldFetchRootKey: local,
  })
  const status = await CanisterStatus.request({
    canisterId: Principal.fromText(wallet),
    agent,
    paths: ["controllers", "module_hash"],
  })
  const controllers = status.get("controllers")
  const moduleHash = status.get("module_hash")
  return {
    controllers: Array.isArray(controllers)
      ? controllers.flatMap((controller) =>
          controller instanceof Principal ? [controller.toText()] : [],
        )
      : [],
    moduleHash: typeof moduleHash === "string" ? moduleHash : undefined,
  }
}
