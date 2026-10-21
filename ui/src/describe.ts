import { isReactorError } from "@ic-reactor/core"
import type { WalletError } from "./generated/wallet.ts"

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
      return "This waits for its timelock."
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
  if (error.mayHaveExecuted) {
    return "The wallet did not answer, and the call may have gone through. The page shows what the wallet says now."
  }
  return error.message
}
