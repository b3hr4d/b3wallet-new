import { useClient } from "@ic-reactor/react"
import { useQuery } from "@tanstack/react-query"
import { actor as ledgerActor, type Actor as Ledger } from "../generated/icrc1.ts"
import type { Chain } from "../generated/wallet.ts"
import {
  bitcoinToken,
  evmToken,
  ICP_LEDGER,
  ledgerOf,
  tokenOf,
  type Token,
} from "../logic/tokens.ts"

export type TokenRead = { token: Token; error?: undefined } | { token?: undefined; error?: unknown }

/**
 * The token a chain's accounts hold: Bitcoin's, an EVM chain's, or what its
 * ledger's metadata says, read as a certified call like every other read.
 */
export function useToken(chain: Chain | undefined): TokenRead {
  const client = useClient()
  const ledger = chain && ledgerOf(chain)
  const metadata = useQuery({
    ...client.queryOptions(
      client.canister<Ledger>(ledgerActor, { id: ledger ?? ICP_LEDGER, certified: true }),
      "icrc1_metadata",
    ),
    enabled: ledger !== undefined,
    staleTime: Infinity,
  })
  if (chain?.tag === "Bitcoin") return { token: bitcoinToken(chain.value) }
  if (chain?.tag === "Evm") return { token: evmToken(chain.value) }
  if (metadata.isError) return { error: metadata.error }
  if (metadata.data === undefined) return {}
  const token = tokenOf(metadata.data)
  return token
    ? { token }
    : { error: new Error("The token's ledger does not say its symbol and decimals.") }
}
