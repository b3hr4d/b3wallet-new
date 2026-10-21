import { principal, type Principal } from "@candid-core/schema"
import { formatUnits, parseUnits } from "@ic-reactor/core"
import { useClient } from "@ic-reactor/react"
import { useQuery } from "@tanstack/react-query"
import {
  actor as ledgerActor,
  type Actor as Ledger,
  type MetadataValue,
} from "./generated/icrc1.ts"
import type { BitcoinNetwork, Chain } from "./generated/wallet.ts"

/** The ICP ledger, at its mainnet id on every network. */
export const ICP_LEDGER = principal("ryjl3-tyaaa-aaaaa-aaaba-cai")

/** What a chain's accounts hold: its symbol and decimals, and a ledger's fee. */
export type Token = { symbol: string; decimals: number; fee: bigint | null }

/** The ledger that holds a chain's tokens, if it has one. */
export function ledgerOf(chain: Chain): Principal | undefined {
  switch (chain.tag) {
    case "Icp":
      return ICP_LEDGER
    case "Icrc":
      return chain.value
    case "Bitcoin":
    case "Evm":
      return undefined
  }
}

/** Bitcoin's token, which no ledger describes. */
export function bitcoinToken(network: BitcoinNetwork): Token {
  const symbol = network.tag === "Mainnet" ? "BTC" : `${network.tag.toLowerCase()} BTC`
  return { symbol, decimals: 8, fee: null }
}

/**
 * The token a ledger's metadata describes, or undefined when it lacks a
 * symbol or decimals that fit.
 */
export function tokenOf(
  metadata: ReadonlyArray<readonly [string, MetadataValue]>,
): Token | undefined {
  const entry = (key: string) => metadata.find(([name]) => name === key)?.[1]
  const symbol = entry("icrc1:symbol")
  const decimals = entry("icrc1:decimals")
  const fee = entry("icrc1:fee")
  if (symbol?.tag !== "Text" || decimals?.tag !== "Nat" || decimals.value > 255n) return undefined
  return {
    symbol: symbol.value,
    decimals: Number(decimals.value),
    fee: fee?.tag === "Nat" ? fee.value : null,
  }
}

/** An amount in a token's base units, as text: `1.5 ICP`. */
export function amountText(amount: bigint, token: Token): string {
  return `${formatUnits(amount, token.decimals)} ${token.symbol}`
}

/**
 * An amount a person typed, in base units of a token with `decimals`
 * decimals: exactly what was typed, or why it is not an amount to send.
 */
export function readAmount(
  text: string,
  decimals: number,
): { amount: bigint; error?: undefined } | { amount?: undefined; error: string } {
  let amount: bigint
  try {
    amount = parseUnits(text, decimals, { signed: true })
  } catch (error) {
    if (error instanceof RangeError) {
      return {
        error:
          decimals === 0
            ? "This token has no decimals: type a whole number."
            : `This token has ${decimals} decimals, so at most ${decimals} digits after the point.`,
      }
    }
    return { error: "Type an amount such as 1.5: digits, with a point before any decimals." }
  }
  return amount > 0n ? { amount } : { error: "Type an amount above zero." }
}

/** A chain, named for a person. */
export function chainText(chain: Chain): string {
  switch (chain.tag) {
    case "Icp":
      return "ICP"
    case "Icrc":
      return `ICRC-1 token of ledger ${chain.value}`
    case "Bitcoin":
      return chain.value.tag === "Mainnet" ? "Bitcoin" : `Bitcoin ${chain.value.tag.toLowerCase()}`
    case "Evm":
      return `EVM chain ${chain.value}`
  }
}

export type TokenRead = { token: Token; error?: undefined } | { token?: undefined; error?: unknown }

/**
 * The token a chain's accounts hold: Bitcoin's, or what its ledger's
 * metadata says, read as a certified call like every other read.
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
  if (metadata.isError) return { error: metadata.error }
  if (metadata.data === undefined) return {}
  const token = tokenOf(metadata.data)
  return token
    ? { token }
    : { error: new Error("The token's ledger does not say its symbol and decimals.") }
}
