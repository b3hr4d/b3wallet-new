import { isReactorError, type ReactorError } from "@ic-reactor/core"
import { AgentError, CertifiedRejectErrorCode } from "@icp-sdk/core/agent"
import type { Account, Action, Pending, Role, TxReceipt, WalletError } from "./generated/wallet.ts"
import { dateOf } from "./time.ts"
import { amountText, type Token } from "./tokens.ts"

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
    case "InsufficientCycles":
      return "The wallet is low on cycles."
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
 * the account it sends from, `from`, and its amount in `token` once they are
 * known, and its base units until then.
 */
export function actionText(
  action: Action,
  from: Account | undefined,
  token: Token | undefined,
): string {
  switch (action.tag) {
    case "Transfer": {
      const { amount, to } = action.value
      const what = token ? amountText(amount, token) : `${amount} base units`
      const source = from ? `“${from.label}”` : `account ${action.value.from}`
      return `Send ${what} from ${source} to ${to}`
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
  }
}

/** The account a transfer sends from, once `accounts` are read. */
export function sourceOf(action: Action, accounts: Account[] | undefined): Account | undefined {
  return action.tag === "Transfer"
    ? accounts?.find((account) => account.id === action.value.from)
    : undefined
}

/** What a join request asks, in a sentence without its full stop. */
export function joinRequestText(pending: Pending): string {
  const label = pending.action.tag === "AddMember" ? pending.action.value.label : ""
  return `${pending.proposed_by} asks to join as a viewer, “${label}”`
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
  return receipt.chain.tag === "Bitcoin"
    ? `Bitcoin transaction ${receipt.tx_id}`
    : `block ${receipt.tx_id} of the ledger`
}
