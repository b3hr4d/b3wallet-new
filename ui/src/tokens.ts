import { principal, type Principal } from "@candid-core/schema"
import { formatUnits, parseUnits } from "@ic-reactor/core"
import { useClient } from "@ic-reactor/react"
import { useQuery } from "@tanstack/react-query"
import {
  actor as ledgerActor,
  type Actor as Ledger,
  type MetadataValue,
} from "./generated/icrc1.ts"
import type { Account, AccountKind, Action, BitcoinNetwork, Chain } from "./generated/wallet.ts"

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

/** The assets every wallet shows: ICP, which is always tracked. */
export const ICP_ONLY: readonly Chain[] = [{ tag: "Icp" }]

/**
 * The EVM chains a wallet holds, those the EVM RPC canister has providers
 * of, by chain id: their names, and their native token's symbol.
 */
export const EVM_CHAINS: ReadonlyArray<{ id: bigint; name: string; symbol: string }> = [
  { id: 1n, name: "Ethereum", symbol: "ETH" },
  { id: 11_155_111n, name: "Sepolia", symbol: "Sepolia ETH" },
  { id: 42_161n, name: "Arbitrum One", symbol: "ETH on Arbitrum One" },
  { id: 8_453n, name: "Base", symbol: "ETH on Base" },
  { id: 10n, name: "Optimism", symbol: "ETH on Optimism" },
]

/**
 * The EVM chains a wallet on `network` holds. A wallet on a test network signs
 * with the test key, which holds Sepolia alone.
 */
export function evmChainsOf(network: BitcoinNetwork): typeof EVM_CHAINS {
  return network.tag === "Mainnet"
    ? EVM_CHAINS
    : EVM_CHAINS.filter((chain) => chain.id === 11_155_111n)
}

/** The native token of an EVM chain: 18 decimals, and the chain's symbol. */
export function evmToken(chain: bigint): Token {
  const known = EVM_CHAINS.find((candidate) => candidate.id === chain)
  return { symbol: known?.symbol ?? `ETH on chain ${chain}`, decimals: 18, fee: null }
}

/**
 * What an account of `kind` reads and sends when a transfer names no asset:
 * ICP, or the bitcoin of its network. An EVM transfer names its chain.
 */
export function defaultAsset(kind: AccountKind): Chain | undefined {
  switch (kind.tag) {
    case "InternetComputer":
      return { tag: "Icp" }
    case "Bitcoin":
      return { tag: "Bitcoin", value: kind.value }
    case "Evm":
      return undefined
  }
}

/**
 * The assets an account shows, its default first: ICP and the tracked
 * ICRC-1 tokens for an Internet Computer account, its network's bitcoin for
 * a Bitcoin account, and the native token of each tracked chain for an EVM
 * account.
 */
export function assetsOf(account: Account, tracked: readonly Chain[]): Chain[] {
  switch (account.kind.tag) {
    case "InternetComputer":
      return tracked.filter((asset) => asset.tag === "Icp" || asset.tag === "Icrc")
    case "Bitcoin":
      return [{ tag: "Bitcoin", value: account.kind.value }]
    case "Evm":
      return tracked.filter((asset) => asset.tag === "Evm")
  }
}

/**
 * The asset a transfer sends: the one it names, or its account's default
 * once the account is known.
 */
export function transferAsset(action: Action, from: Account | undefined): Chain | undefined {
  if (action.tag !== "Transfer") return undefined
  return action.value.asset ?? (from && defaultAsset(from.kind))
}

/** An account's kind, named for a person. */
export function kindText(kind: AccountKind): string {
  switch (kind.tag) {
    case "InternetComputer":
      return "Internet Computer"
    case "Bitcoin":
      return chainText({ tag: "Bitcoin", value: kind.value })
    case "Evm":
      return "EVM"
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

/**
 * An amount on a chain, as text, where no account names the token: ICP,
 * Bitcoin and an EVM chain's token in their symbols, another ledger's token
 * in its base units.
 */
export function chainAmountText(amount: bigint, chain: Chain): string {
  switch (chain.tag) {
    case "Icp":
      return amountText(amount, { symbol: "ICP", decimals: 8, fee: null })
    case "Bitcoin":
      return amountText(amount, bitcoinToken(chain.value))
    case "Evm":
      return amountText(amount, evmToken(chain.value))
    case "Icrc":
      return `${amount} base units`
  }
}

/** A hundredth of a trillion cycles, the step `cyclesText` rounds to. */
const CENTI_TRILLION = 10_000_000_000n

/**
 * Cycles in trillions to two decimals, rounded `down` or `up`: "0.29T
 * cycles".
 */
export function cyclesText(cycles: bigint, round: "down" | "up"): string {
  const rest = cycles % CENTI_TRILLION
  const up = round === "up" && rest > 0n ? CENTI_TRILLION : 0n
  return `${formatUnits(cycles - rest + up, 12)}T cycles`
}

/**
 * Below this many cycles the pages say the wallet is low. An upgrade needs
 * about 0.35T above what the wallet keeps to stay running, so a wallet under
 * 1T is told before an upgrade stops for want of cycles.
 */
export const LOW_CYCLES = 1_000_000_000_000n

/** An amount in a token's base units, as text: `1.5 ICP`. */
export function amountText(amount: bigint, token: Token): string {
  return `${formatUnits(amount, token.decimals)} ${token.symbol}`
}

/**
 * An amount split to show large: the whole part with its thousands grouped,
 * the fraction with its point, and the symbol. Every digit stays.
 */
export function amountParts(
  amount: bigint,
  token: Token,
): { whole: string; fraction: string; symbol: string } {
  const [whole = "0", fraction] = formatUnits(amount, token.decimals).split(".")
  return {
    whole: whole.replace(/\B(?=(\d{3})+(?!\d))/g, ","),
    fraction: fraction === undefined ? "" : `.${fraction}`,
    symbol: token.symbol,
  }
}

/** One text per chain, the same for every account of it: what totals group by. */
export function chainKey(chain: Chain): string {
  switch (chain.tag) {
    case "Icp":
      return "Icp"
    case "Icrc":
      return `Icrc ${chain.value}`
    case "Bitcoin":
      return `Bitcoin ${chain.value.tag}`
    case "Evm":
      return `Evm ${chain.value}`
  }
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
      return (
        EVM_CHAINS.find((candidate) => candidate.id === chain.value)?.name ??
        `EVM chain ${chain.value}`
      )
  }
}

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
