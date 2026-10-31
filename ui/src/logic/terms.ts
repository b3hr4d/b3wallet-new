import type { Grant } from "@meantjs/consent"
import {
  checkGrant,
  confirmTerms,
  readTerms,
  termsText,
  type ConfirmedTerms,
  type Terms,
  type Wallet,
} from "@meantjs/consent/terms"
import { displayAmount, normalizeName, parseAccount } from "@meantjs/format"
import { jcs } from "@meantjs/profile/evm"
import type {
  Account,
  Action,
  ConfirmTermsArgs,
  RequestTerms,
  TermsConfirmation,
} from "../generated/wallet.ts"
import { ICP_LEDGER, ledgerOf, transferAsset, type Token } from "./tokens.ts"

// Terms of a request (decision 0014): the agent reads the token, the
// recipient, the most and the latest end in an owner's words, an owner
// confirms them on this page, and the Approvals page checks the agent's
// transfer against them. Reading the terms, writing them in words and the
// check are @meantjs/consent/terms; this file only says what the wallet
// knows, and turns a transfer into the grant the check reads.

/** A token terms may name: a ledger the wallet's Internet Computer accounts hold. */
export type LedgerToken = { ledger: string; token: Token }

/** How the terms look a token or an account up: NFKC, lowercase, letters and digits only. */
function nameKey(name: string): string {
  return normalizeName(name).replace(/[^\p{L}\p{N}]/gu, "")
}

function counts(keys: readonly string[]): Map<string, number> {
  const seen = new Map<string, number>()
  for (const key of keys) seen.set(key, (seen.get(key) ?? 0) + 1)
  return seen
}

function isAccount(text: string): boolean {
  try {
    parseAccount(text)
    return true
  } catch {
    return false
  }
}

/**
 * What the terms read against: the tokens of `tokens` by symbol and the
 * wallet's own Internet Computer accounts by label, at `now`, in a zone
 * `utcOffsetMinutes` east of UTC. A name that would not name one thing is
 * left out, so terms cannot use it: one with no letters or digits, a symbol
 * @meantjs/format would not show, and a name two tokens or two accounts
 * share once case, spaces and punctuation are set aside. ICP keeps its name,
 * and another ledger that calls itself ICP is left out.
 */
export function termsWallet(
  tokens: readonly LedgerToken[],
  accounts: readonly Account[],
  now: bigint,
  utcOffsetMinutes: number,
): Wallet {
  const shown = tokens.filter(({ ledger, token }, index) => {
    if (tokens.findIndex((other) => other.ledger === ledger) !== index) return false
    try {
      displayAmount(0n, token)
    } catch {
      return false
    }
    return nameKey(token.symbol) !== ""
  })
  // ICP's names are ICP's, whether or not its ledger could be read: "ICP", and the symbol its ledger reports.
  const icp = shown.find(({ ledger }) => ledger === ICP_LEDGER)
  const reserved = new Set([nameKey("ICP"), ...(icp ? [nameKey(icp.token.symbol)] : [])])
  const symbols = counts(shown.map(({ token }) => nameKey(token.symbol)))
  const known = shown.filter((entry) => {
    const key = nameKey(entry.token.symbol)
    return entry === icp || (!reserved.has(key) && symbols.get(key) === 1)
  })
  const own = accounts.filter(
    (account) =>
      account.kind.tag === "InternetComputer" &&
      nameKey(account.label) !== "" &&
      isAccount(account.address),
  )
  const labels = counts(own.map((account) => nameKey(account.label)))
  return {
    tokens: Object.fromEntries(
      known.map(({ ledger, token }) => [
        token.symbol,
        { canister: ledger, decimals: token.decimals },
      ]),
    ),
    apps: Object.fromEntries(
      own
        .filter((account) => labels.get(nameKey(account.label)) === 1)
        .map((account) => [account.label, account.address]),
    ),
    now,
    utcOffsetMinutes,
  }
}

/** The browser's zone at `ns`, in whole minutes east of UTC, as the terms take it. */
export function zoneAt(ns: bigint): number {
  const minutes = -new Date(Number(ns / 1_000_000n)).getTimezoneOffset()
  return minutes === 0 ? 0 : minutes
}

export type DraftRead =
  | { terms: Terms; confirmation: ConfirmTermsArgs; error?: undefined }
  | { terms?: undefined; confirmation?: undefined; error: string }

/**
 * The agent's draft, read as of when it was written, in the zone of
 * `utcOffsetMinutes` (the browser's then): the terms, and what the page
 * sends when an owner confirms them, their words included. Or why they
 * cannot be read: the library never guesses a token, a recipient, decimals
 * or a date, and its reason is written for the agent, which writes them
 * again once an owner says no.
 */
export function readDraft(
  terms: RequestTerms,
  tokens: readonly LedgerToken[],
  accounts: readonly Account[],
  utcOffsetMinutes: number = zoneAt(terms.drafted_at_ns),
): DraftRead {
  const wallet = termsWallet(tokens, accounts, terms.drafted_at_ns, utcOffsetMinutes)
  try {
    const read = readTerms(
      {
        token: terms.draft.token,
        to: terms.draft.to,
        max_amount: terms.draft.max_amount,
        expires_by: terms.draft.expires_by,
      },
      wallet,
    )
    return { terms: read, confirmation: confirmationOf(read, terms.version) }
  } catch (error) {
    return { error: error instanceof Error ? error.message : String(error) }
  }
}

/** When terms end, in nanoseconds since 1970, or null for terms that never end. */
export function endOf(terms: Terms): bigint | null {
  if (!("at" in terms.ends)) return null
  const [whole = "0", fraction = ""] = terms.ends.at.split(".")
  return BigInt(whole) * 1_000_000_000n + BigInt(fraction.padEnd(9, "0"))
}

/**
 * What the page sends when an owner confirms version `version` of a
 * request's terms: the terms as JSON (RFC 8785), their id, which is the
 * SHA-256 of that JSON, and the words the owner read.
 */
export function confirmationOf(terms: Terms, version: number): ConfirmTermsArgs {
  const confirmed = confirmTerms(terms)
  const { id, ...rest } = confirmed
  return { version, terms: jcs(rest), id, text: termsText(confirmed) }
}

/**
 * The terms an owner confirmed, as the wallet holds them, or undefined when
 * they cannot be read: JSON that is not terms the library wrote, or terms
 * whose id is not the SHA-256 of their JSON.
 */
export function confirmedTerms(record: TermsConfirmation): ConfirmedTerms | undefined {
  let parsed: unknown
  try {
    parsed = JSON.parse(record.terms)
  } catch {
    return undefined
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) return undefined
  if (Object.hasOwn(parsed, "id")) return undefined
  const confirmed = { ...parsed, id: record.id } as ConfirmedTerms
  try {
    // termsText refuses terms the library did not write, and terms whose id is not theirs.
    termsText(confirmed)
  } catch {
    return undefined
  }
  return confirmed
}

/**
 * What a transfer grants, as @meantjs/consent writes a grant: its recipient
 * is paid directly, from a ledger, when it runs. Null for a transfer no
 * terms describe: bitcoin, an EVM chain's token, or one whose account is not
 * read yet.
 */
export function transferGrant(action: Action, from: Account | undefined): Grant | null {
  if (action.tag !== "Transfer") return null
  const asset = transferAsset(action, from)
  const ledger = asset && ledgerOf(asset)
  if (ledger === undefined) return null
  return {
    kind: "transfer",
    to: action.value.to,
    token: ledger,
    amount: { units: action.value.amount.toString(), unlimited: false },
    ends: { now: true },
  }
}

/**
 * Whether a transfer fits the terms an owner confirmed, at `now`, and if not,
 * why, in words. Names in the reasons are the wallet's own accounts.
 */
export function checkTransfer(
  confirmed: ConfirmedTerms,
  action: Action,
  from: Account | undefined,
  accounts: readonly Account[],
  now: bigint,
): { fits: boolean; reasons: string[] } {
  const token = { symbol: confirmed.symbol, decimals: confirmed.decimals, fee: null }
  const wallet = termsWallet(
    [{ ledger: confirmed.token, token }],
    accounts,
    now,
    confirmed.utcOffsetMinutes,
  )
  try {
    return checkGrant(confirmed, transferGrant(action, from), wallet)
  } catch (error) {
    const why = error instanceof Error ? error.message : String(error)
    return { fits: false, reasons: [`it cannot be checked against them: ${why}`] }
  }
}
