import type { Member, Policy, Role } from "../generated/wallet.ts"

/** The longest member label the wallet takes, in bytes of UTF-8. */
export const MAX_LABEL_BYTES = 64

/** A change to the members: someone removed, or given another role. */
export type MemberChange = { remove: string } | { id: string; role: Role }

/** Whether a role's approval counts, as the wallet's policy module decides. */
export function approves(role: Role): boolean {
  return role.tag === "Owner" || role.tag === "Approver"
}

/**
 * Why the wallet would refuse `change` to `members` under `policy`, as its
 * governance check does (`check_governance` in the canister): it must keep an
 * owner, and as many owners and approvers as the policy's largest quorum. The
 * page says so before an owner proposes, instead of after; the wallet checks
 * again whatever the page says.
 */
export function changeRefusal(
  members: readonly Member[],
  policy: Policy | undefined,
  change: MemberChange,
): string | undefined {
  const roles = members.flatMap((member) =>
    "remove" in change
      ? member.id === change.remove
        ? []
        : [member.role]
      : [member.id === change.id ? change.role : member.role],
  )
  if (!roles.some((role) => role.tag === "Owner")) return "The wallet must keep an owner."
  const quorum = Math.max(1, ...(policy?.rules ?? []).map((rule) => rule.quorum))
  const left = roles.filter(approves).length
  if (quorum > left) {
    return `The policy asks for ${quorum} approvals, so the wallet must keep ${quorum} owners and approvers.`
  }
  return undefined
}

/** Why a label is not one the wallet takes, if it is not. */
export function labelRefusal(label: string): string | undefined {
  if (!label.trim()) return "Give it a name."
  if (new TextEncoder().encode(label).length > MAX_LABEL_BYTES) {
    return `A name has at most ${MAX_LABEL_BYTES} bytes.`
  }
  return undefined
}
