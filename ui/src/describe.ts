import { isReactorError, type ReactorError } from "@ic-reactor/core"
import { AgentError, CertifiedRejectErrorCode } from "@icp-sdk/core/agent"
import type {
  Account,
  AccountKind,
  Action,
  ActionKind,
  AuditEvent,
  Budget,
  Chain,
  Member,
  Pending,
  Permission,
  PolicyRule,
  Role,
  TxReceipt,
  WalletError,
} from "./generated/wallet.ts"
import { dateOf, durationText } from "./time.ts"
import { amountText, chainAmountText, chainText, cyclesText, type Token } from "./tokens.ts"

/** What a refusal of the wallet means, in a sentence. */
export function walletErrorText(error: WalletError): string {
  switch (error.tag) {
    case "AlreadyClaimed":
      return "This wallet is already claimed."
    case "ClaimExpired":
      return "This claim link has expired. Ask whoever created the wallet for a new one."
    case "BadSignature":
      return "This claim link does not belong to this wallet."
    case "NotClaimed":
      return "This wallet is not claimed yet."
    case "NotAMember":
      return "You are not a member of this wallet."
    case "UpgradeInProgress":
      return "The wallet is installing a new version. Try again in a few minutes."
    case "InsufficientCycles": {
      const { available, needed } = error.value
      return `The wallet has ${cyclesText(available, "down")} to spend, and this needs ${cyclesText(needed, "up")}. Send it cycles, then try again.`
    }
    case "QuorumNotMet":
      return `This needs ${error.value.quorum} approvals and has ${error.value.approvals}.`
    case "TimelockActive":
      return `This can run after ${dateOf(error.value.executable_after_ns)}.`
    case "BudgetExceeded":
      return "This exceeds the remaining budget."
    case "NotFound":
    case "InvalidArgument":
    case "ChainError":
    case "PolicyViolation":
    case "ReleaseRejected":
    case "Forbidden":
      return error.value
  }
}

/**
 * What went wrong with a call, in a sentence. A write that may have run says
 * so, since sending it again could run it twice.
 */
export function failureText(error: unknown): string {
  if (!isReactorError(error)) return error instanceof Error ? error.message : String(error)
  if (error.kind === "canister_err") return walletErrorText(error.err as WalletError)
  const refusal = refusalOf(error)
  if (refusal !== undefined) return `Refused: ${refusal}.`
  if (error.mayHaveExecuted) {
    return "The wallet did not answer, and the call may have gone through. The page shows what the wallet says now."
  }
  return error.message
}

/**
 * Why a canister refused a call, when it did. The app writes only to the
 * wallet, which rejects a call only in a guard, before the method runs (see
 * `canisters/wallet/src/access.rs`): a write it rejected did nothing, whatever
 * an update's reject leaves open in general. Only a certified reject counts;
 * every call the app makes is replicated, so no single replica speaks here.
 */
function refusalOf(error: ReactorError): string | undefined {
  const cause = error.cause
  if (
    error.kind === "rejected" &&
    error.rejectCode === 4 &&
    cause instanceof AgentError &&
    cause.code instanceof CertifiedRejectErrorCode
  ) {
    return cause.code.rejectMessage
  }
  return undefined
}

/** A role, after "is" or "as". */
export function roleText(role: Role): string {
  switch (role.tag) {
    case "Owner":
      return "an owner"
    case "Approver":
      return "an approver"
    case "Viewer":
      return "a viewer"
    case "Proposer":
      return "a proposer"
  }
}

/**
 * Whether an action is a join request: an `AddMember` of its own proposer,
 * which only someone who is not a member yet sends.
 */
export function isJoinRequest(pending: Pending): boolean {
  return pending.action.tag === "AddMember" && pending.action.value.id === pending.proposed_by
}

/**
 * What an action does, in a sentence without its full stop. A transfer names
 * the account it sends from, `from`, and its amounts in `token` once they are
 * known, and in base units until then. It names a fee set in the proposal: a
 * Bitcoin transfer spends it on top of the amount, and an EVM transfer pays
 * at most it.
 */
export function actionText(
  action: Action,
  from: Account | undefined,
  token: Token | undefined,
): string {
  switch (action.tag) {
    case "Transfer": {
      const { amount, to, fee } = action.value
      const units = (value: bigint) => (token ? amountText(value, token) : `${value} base units`)
      const source = from ? `“${from.label}”` : `account ${action.value.from}`
      const paying =
        fee === null
          ? ""
          : action.value.asset?.tag === "Evm"
            ? `, paying at most ${units(fee)} in fees`
            : `, paying a fee of ${units(fee)}`
      return `Send ${units(amount)} from ${source} to ${to}${paying}`
    }
    case "AddMember":
      return `Add ${action.value.id} as ${roleText(action.value.role)}, “${action.value.label}”`
    case "RemoveMember":
      return `Remove the member ${action.value}`
    case "ChangeRole":
      return `Make ${action.value.id} ${roleText(action.value.role)}`
    case "InstallExtension":
      return `Install the extension ${action.value.canister}`
    case "UninstallExtension":
      return `Uninstall the extension ${action.value}`
    case "SetPolicy":
      return "Change the approval policy"
    case "Upgrade":
      return `Upgrade the wallet to version ${action.value.version}`
    case "HandOverControl":
      return `Hand control of the wallet's code to the recovery controller ${action.value.controller}`
  }
}

/**
 * What an action sets beyond what `actionText` says, a sentence a line, so a
 * member sees all of it before approving: a transfer's memo, an extension's
 * permissions and budgets, a policy's rules and an upgrade's Wasm hash.
 */
export function actionDetails(action: Action): string[] {
  switch (action.tag) {
    case "Transfer":
      return action.value.memo === null ? [] : [`Memo: ${hexOf(action.value.memo)}.`]
    case "InstallExtension": {
      const { granted, budgets } = action.value
      if (granted.length === 0 && budgets.length === 0) return ["It is granted nothing."]
      return [...granted.map(permissionText), ...budgets.map(budgetText)]
    }
    case "SetPolicy":
      return action.value.rules.flatMap(ruleText)
    case "Upgrade":
      return [`The new version's Wasm has the SHA-256 ${hexOf(action.value.wasm_sha256)}.`]
    case "HandOverControl":
      return [
        "After it, only the recovery keys that controller holds can change the wallet's code, and the wallet can never take control back.",
        `The controller's module has the SHA-256 ${hexOf(action.value.controller_module_hash)}.`,
        `Its recovery policy has the SHA-256 ${hexOf(action.value.controller_policy_sha256)}.`,
      ]
    case "AddMember":
    case "RemoveMember":
    case "ChangeRole":
    case "UninstallExtension":
      return []
  }
}

function permissionText(permission: Permission): string {
  switch (permission.tag) {
    case "ReadAccounts":
      return "It may read the accounts and their balances."
    case "ProposeTransfer":
      return "It may propose transfers."
    case "SpendSubWallet":
      return `It may spend from its own sub-wallet on ${chainText(permission.value)}.`
  }
}

function budgetText(budget: Budget): string {
  const perDay = `${chainAmountText(budget.per_day, budget.chain)} a day`
  const total =
    budget.total === null ? "" : ` and ${chainAmountText(budget.total, budget.chain)} in all`
  return `On ${chainText(budget.chain)}, it may spend ${perDay}${total}.`
}

function ruleText(rule: PolicyRule): string[] {
  const approvals = `${rule.quorum} approval${rule.quorum === 1 ? "" : "s"}`
  const wait = rule.timelock_ns === 0n ? "no wait" : `a wait of ${durationText(rule.timelock_ns)}`
  return [
    `${KIND_TEXT[rule.kind.tag]}: ${approvals} and ${wait}.`,
    ...rule.daily_limits.map(
      (limit) =>
        `A transfer that keeps the last 24 hours' spending on ${chainText(limit.chain)} within ${chainAmountText(limit.amount, limit.chain)} needs only its proposer and no wait.`,
    ),
  ]
}

const KIND_TEXT: Record<ActionKind["tag"], string> = {
  Transfer: "A transfer",
  AddMember: "Adding a member",
  RemoveMember: "Removing a member",
  ChangeRole: "Changing a role",
  InstallExtension: "Installing an extension",
  UninstallExtension: "Uninstalling an extension",
  SetPolicy: "Changing the policy",
  Upgrade: "An upgrade",
}

function hexOf(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("")
}

/** The account a transfer sends from, once `accounts` are read. */
export function sourceOf(action: Action, accounts: Account[] | undefined): Account | undefined {
  return action.tag === "Transfer"
    ? accounts?.find((account) => account.id === action.value.from)
    : undefined
}

/** What a join request asks, in a sentence without its full stop. */
export function joinRequestText(pending: Pending): string {
  if (pending.action.tag !== "AddMember") return ""
  const { role, label } = pending.action.value
  return `${pending.proposed_by} asks to join as ${roleText(role)}, “${label}”`
}

/**
 * The browser's clock may run a little behind the wallet's. An action whose
 * timelock ends within this margin counts as ready to run; if it is still
 * early, the wallet refuses and says until when.
 */
const CLOCK_MARGIN_NS = 60_000_000_000n

/** Whether an action can run at `nowNs`, within the clocks' margin. */
export function canRun(pending: Pending, nowNs: bigint): boolean {
  return pending.state.tag === "Ready" && pending.executable_after_ns <= nowNs + CLOCK_MARGIN_NS
}

/** Where an action stands at `nowNs`, in a sentence. */
export function stateText(pending: Pending, nowNs: bigint): string {
  const { state } = pending
  switch (state.tag) {
    case "Waiting":
      return `Waiting for approvals: ${pending.approvals.length} of ${pending.quorum}.`
    case "Ready":
      return canRun(pending, nowNs)
        ? "Approved and ready to run."
        : `Approved. It can run after ${dateOf(pending.executable_after_ns)}.`
    case "Executed":
      return state.value.tag === "Done" ? "Done." : `Sent: ${receiptText(state.value.value)}.`
    case "Rejected":
      return `Rejected by ${state.value}.`
    case "Expired":
      return "Expired."
    case "Failed":
      return `Failed: ${state.value}`
  }
}

/** Where a transfer that was sent can be found. */
export function receiptText(receipt: TxReceipt): string {
  switch (receipt.chain.tag) {
    case "Bitcoin":
      return `Bitcoin transaction ${receipt.tx_id}`
    case "Evm":
      return `transaction ${receipt.tx_id} on ${chainText(receipt.chain)}`
    case "Icp":
    case "Icrc":
      return `block ${receipt.tx_id} of the ledger`
  }
}

/**
 * Whether an action waits for `me`: one they may approve and have not, or
 * one approved that they may run now. Only owners and approvers do either.
 */
export function waitsFor(pending: Pending, me: Member, nowNs: bigint): boolean {
  if (me.role.tag !== "Owner" && me.role.tag !== "Approver") return false
  if (pending.state.tag === "Waiting") return !pending.approvals.includes(me.id)
  return canRun(pending, nowNs)
}

/** A long id or address cut to its ends, for where the whole one does not fit. */
export function shortText(text: string, head = 6, tail = 4): string {
  return text.length <= head + tail + 1 ? text : `${text.slice(0, head)}…${text.slice(-tail)}`
}

/**
 * Who did what the audit log records: “You”, a member's name, or a
 * principal cut short; the wallet itself for what its timers do.
 */
export function whoText(
  by: string | null,
  me: Member,
  members: readonly Member[] | undefined,
): string {
  if (by === null) return "The wallet"
  if (by === me.id) return "You"
  const label = members?.find((member) => member.id === by)?.label
  return label ? label : shortText(by)
}

/** A tracked asset, to follow "tracked": the ICP, or Base. */
function trackedText(asset: Chain): string {
  return asset.tag === "Evm" ? chainText(asset) : `the ${chainText(asset)}`
}

/** What an audit event did, to follow who did it: `proposed action 4`. */
export function auditText(event: AuditEvent, accounts: readonly Account[] | undefined): string {
  switch (event.tag) {
    case "Claimed":
      return "claimed the wallet"
    case "JoinRequested":
      return "asked to join"
    case "AccountAdded": {
      const account = accounts?.find((candidate) => candidate.id === event.value)
      return account ? `added the account “${account.label}”` : `added account ${event.value}`
    }
    case "AssetTracked":
      return `tracked ${trackedText(event.value)}`
    case "AssetUntracked":
      return `stopped tracking ${trackedText(event.value)}`
    case "Proposed":
      return `proposed action ${event.value}`
    case "Approved":
      return `approved action ${event.value}`
    case "Rejected":
      return `rejected action ${event.value}`
    case "Executed":
      return `ran action ${event.value}`
    case "ExtensionCall":
      return `asked for ${event.value.request}, as an extension`
    case "UpgradeStaged":
      return `staged version ${event.value}`
    case "UpgradeInstalled":
      return `installed version ${event.value}`
    case "UpgradeFailed":
      return `failed to install version ${event.value}`
    case "CyclesLow":
      return `ran low on cycles, with ${cyclesText(event.value, "down")} left`
  }
}

/**
 * An address in groups to read aloud and check: a principal or an ICRC-1
 * account already reads in its dash-separated groups, so it stays whole;
 * anything else, such as a Bitcoin address, goes in fours. Joined, the
 * groups are the address again.
 */
export function addressGroups(address: string): string[] {
  return address.includes("-") ? [address] : (address.match(/.{1,4}/g) ?? [])
}

/**
 * Which form of an Internet Computer account's address a sender gets: its
 * ICRC-1 account, or the ICP account identifier exchanges ask for.
 */
export type AddressForm = "account" | "identifier"

/**
 * What a sender must know about an account before sending to it: what it
 * counts, on which network, and what becomes of anything else.
 */
export function receiveNote(kind: AccountKind, form: AddressForm = "account"): string {
  switch (kind.tag) {
    case "InternetComputer":
      return form === "identifier"
        ? "For ICP only, from an exchange that asks for a 64-character account identifier. Send ICRC-1 tokens to the ICRC-1 account instead."
        : "Send ICP or an ICRC-1 token the wallet tracks here, on the Internet Computer. Another ICRC-1 token sent here is held too, and shows once an owner tracks its ledger."
    case "Bitcoin":
      return kind.value.tag === "Mainnet"
        ? "Send only bitcoin here, on the Bitcoin network. A payment counts in the balance once it has a confirmation."
        : `Send only ${kind.value.tag.toLowerCase()} bitcoin here, never mainnet's. A payment counts in the balance once it has a confirmation.`
    case "Evm":
      return "Send only ETH here, on an EVM chain the wallet tracks; the address is the same on each. The wallet shows no ERC-20 token, such as USDC."
  }
}
