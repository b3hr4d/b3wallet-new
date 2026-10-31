import { useClient } from "@ic-reactor/react"
import { useQueries, useQuery } from "@tanstack/react-query"
import { actor as ledgerActor, type Actor as Ledger } from "../generated/icrc1.ts"
import type { Chain } from "../generated/wallet.ts"
import type { LedgerToken } from "../logic/terms.ts"
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

/**
 * The tokens terms may name (decision 0014): those of the ledgers among
 * `assets`, ICP and each tracked ICRC-1 token, as each ledger's metadata
 * describes it, once every ledger has answered; undefined until then. A
 * ledger that could not be read, or that does not say its symbol and
 * decimals, is left out.
 */
export function useLedgerTokens(assets: readonly Chain[] | undefined): LedgerToken[] | undefined {
  const client = useClient()
  const ledgers = (assets ?? []).flatMap((asset) => ledgerOf(asset) ?? [])
  const reads = useQueries({
    queries: ledgers.map((id) => ({
      ...client.queryOptions(
        client.canister<Ledger>(ledgerActor, { id, certified: true }),
        "icrc1_metadata",
      ),
      staleTime: Infinity,
    })),
  })
  if (assets === undefined || reads.some((read) => read.isPending)) return undefined
  return ledgers.flatMap((ledger, index) => {
    const metadata = reads[index]?.data
    const token = metadata && tokenOf(metadata)
    return token ? [{ ledger, token }] : []
  })
}
