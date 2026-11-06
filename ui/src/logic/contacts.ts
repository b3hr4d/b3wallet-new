import type { Account, AccountKind, Action, Contact } from "../generated/wallet.ts"
import { kindText, transferAsset } from "./tokens.ts"

// The owners' contacts (decision 0015): a name and at most one address per
// kind of account. Adding one or changing an address waits out the
// SetContact rule's timelock, so the start page shows exactly what
// changes, and warns when an address looks like one the owners already pay.

/**
 * Whether a contact's name is plain English, as the wallet takes it in the
 * first version: printable ASCII with at least one letter (`contacts.rs`
 * `plain_name`).
 */
export function plainName(name: string): boolean {
  return /^[\x20-\x7e]*$/.test(name) && /[A-Za-z]/.test(name)
}

export function sameKind(a: AccountKind, b: AccountKind): boolean {
  if (a.tag !== b.tag) return false
  return a.tag !== "Bitcoin" || b.tag !== "Bitcoin" || a.value.tag === b.value.tag
}

/** How addresses compare: an EVM address with its case aside, any other as it is written. */
function addressKey(address: string): string {
  return /^0x[0-9a-fA-F]{40}$/.test(address) ? address.toLowerCase() : address
}

/**
 * Whether `a` looks like `b` without being it: the same first 6 and last 4
 * characters and something different between, as address poisoning makes
 * one (candid-profile's `lookalike` rule, `guard/addresses.py`).
 */
export function looksLike(a: string, b: string): boolean {
  const [x, y] = [addressKey(a), addressKey(b)]
  if (x === y || x.length <= 10 || y.length <= 10) return false
  return x.slice(0, 6) === y.slice(0, 6) && x.slice(-4) === y.slice(-4)
}

/** One kind's address of a contact before and after a `SetContact`; absent where it has none. */
export type AddressChange = {
  kind: AccountKind
  before?: string | undefined
  after?: string | undefined
}

/** What a contact action changes, with the contact it changes once the contacts are read. */
export type ContactChange = {
  contact?: Contact | undefined
  name?: { before?: string | undefined; after: string } | undefined
  addresses: AddressChange[]
  /** Addresses that look like another contact's or the wallet's own, in sentences. */
  warnings: string[]
}

/**
 * What a `SetContact` or a `RemoveContact` changes, each kind's old and new
 * address side by side, and a warning for each new address that looks like
 * one of another contact's or of the wallet's own accounts. Undefined for
 * another action.
 */
export function contactChange(
  action: Action,
  contacts: readonly Contact[],
  accounts: readonly Account[],
): ContactChange | undefined {
  if (action.tag === "RemoveContact") {
    const contact = contacts.find((contact) => contact.id === action.value)
    return {
      contact,
      addresses: (contact?.addresses ?? []).map((address) => ({
        kind: address.kind,
        before: address.address,
      })),
      warnings: [],
    }
  }
  if (action.tag !== "SetContact") return undefined
  const { id, name, addresses } = action.value
  const contact = id === null ? undefined : contacts.find((contact) => contact.id === id)
  const kinds: AccountKind[] = []
  for (const address of [...(contact?.addresses ?? []), ...addresses]) {
    if (!kinds.some((kind) => sameKind(kind, address.kind))) kinds.push(address.kind)
  }
  const of = (list: readonly { kind: AccountKind; address: string }[], kind: AccountKind) =>
    list.find((address) => sameKind(address.kind, kind))?.address
  const warnings: string[] = []
  for (const address of addresses) {
    const known = [
      ...contacts
        .filter((other) => other.id !== id)
        .flatMap((other) =>
          other.addresses.map((theirs) => ({ address: theirs.address, whose: `“${other.name}”` })),
        ),
      ...accounts.map((account) => ({
        address: account.address,
        whose: `the wallet's own “${account.label}”`,
      })),
    ]
    for (const { address: theirs, whose } of known) {
      if (looksLike(address.address, theirs)) {
        warnings.push(
          `The ${kindText(address.kind)} address ${address.address} looks like ${whose} ${theirs}: the same first 6 and last 4 characters, and different between. Check every character with the person before approving.`,
        )
      }
    }
  }
  return {
    contact,
    name: contact?.name === name ? undefined : { before: contact?.name, after: name },
    addresses: kinds.map((kind) => ({
      kind,
      before: contact && of(contact.addresses, kind),
      after: of(addresses, kind),
    })),
    warnings,
  }
}

/**
 * Who a transfer pays, as the owners know them: a contact by name, one of the
 * wallet's own accounts by label, or an address that is neither. Undefined
 * for another action, or before the accounts are read.
 */
export function recipientText(
  action: Action,
  from: Account | undefined,
  accounts: readonly Account[] | undefined,
  contacts: readonly Contact[] | undefined,
): string | undefined {
  if (action.tag !== "Transfer" || accounts === undefined || contacts === undefined) return
  const to = addressKey(action.value.to)
  const own = accounts.find((account) => addressKey(account.address) === to)
  if (own) return `To the wallet's own account “${own.label}”.`
  const kind = from?.kind ?? kindOfAsset(transferAsset(action, from))
  const contact = contacts.find((contact) =>
    contact.addresses.some(
      (address) =>
        (kind === undefined || sameKind(address.kind, kind)) && addressKey(address.address) === to,
    ),
  )
  if (contact) return `To ${contact.name} (contact).`
  return "To an address that is not a contact and not one of the wallet's accounts."
}

function kindOfAsset(asset: ReturnType<typeof transferAsset>): AccountKind | undefined {
  switch (asset?.tag) {
    case "Icp":
    case "Icrc":
      return { tag: "InternetComputer" }
    case "Bitcoin":
      return { tag: "Bitcoin", value: asset.value }
    case "Evm":
      return { tag: "Evm" }
    default:
      return undefined
  }
}
