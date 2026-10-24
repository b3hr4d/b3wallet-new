import type { Member, Policy } from "../generated/wallet.ts"
import type { Check } from "./health.ts"
import { durationText } from "./time.ts"
import { chainAmountText } from "./tokens.ts"

const DAY_NS = 86_400_000_000_000n

/**
 * The security checkup on the settings page: what protects the funds if one
 * owner's device or session is taken, and what still does not. Each check is
 * ok, needs attention, or is a problem, as the health dot's checks are.
 * `controllers` is what the IC's certified state says, once read.
 */
export function checkupOf({
  wallet,
  members,
  policy,
  controllers,
}: {
  wallet: string
  members: readonly Member[] | undefined
  policy: Policy | undefined
  controllers: readonly string[] | undefined
}): Check[] {
  const checks: Check[] = []
  if (controllers !== undefined) {
    const others = controllers.filter((controller) => controller !== wallet)
    checks.push(
      others.length === 0 && controllers.includes(wallet)
        ? {
            level: "ok",
            title: "Its own only controller",
            text: "Nobody else can install code on it: it installs only releases its owners approve.",
          }
        : {
            level: "problem",
            title: "Others control it",
            text: `${others.join(", ") || "Nobody"} can install any code on it, and so move every fund.`,
          },
    )
  }
  if (members !== undefined) {
    const owners = members.filter((member) => member.role.tag === "Owner").length
    checks.push(
      owners > 1
        ? { level: "ok", title: "A second owner", text: `${owners} owners can each keep it going.` }
        : {
            level: "attention",
            title: "One owner",
            text: "Nothing recovers a wallet whose last owner is lost. Add a second device, then make it an owner.",
            href: "/members",
          },
    )
  }
  if (policy !== undefined) {
    const rule = (kind: string) => policy.rules.find((candidate) => candidate.kind.tag === kind)
    const transfer = rule("Transfer")
    if (transfer) {
      const limits = transfer.daily_limits
        .map((limit) => chainAmountText(limit.amount, limit.chain))
        .join(", ")
      checks.push(
        transfer.quorum > 1
          ? {
              level: "ok",
              title: `Transfers need ${transfer.quorum} approvals`,
              text: limits
                ? `Above a day's ${limits}; below it, an owner sends alone.`
                : "Every transfer, whatever its amount.",
            }
          : {
              level: "attention",
              title: "One approval sends any amount",
              text: "A stolen device or session could empty the wallet at once. Ask for two approvals above a daily limit.",
            },
      )
    }
    const adding = rule("AddMember")
    if (adding) {
      checks.push(
        adding.timelock_ns > 0n || adding.quorum > 1
          ? {
              level: "ok",
              title: "Adding a member is guarded",
              text: `${adding.quorum} approval${adding.quorum === 1 ? "" : "s"}${
                adding.timelock_ns === 0n
                  ? ", and no wait."
                  : `, then a wait of ${durationText(adding.timelock_ns)}.`
              }`,
            }
          : {
              level: "attention",
              title: "Adding a member needs no wait",
              text: "A stolen session could add an owner of its own at once, and keep its access.",
            },
      )
    }
    const upgrade = rule("Upgrade")
    if (upgrade) {
      checks.push(
        upgrade.timelock_ns >= DAY_NS
          ? {
              level: "ok",
              title: "Upgrades wait",
              text: `${durationText(upgrade.timelock_ns)} to see and reject a release before it installs.`,
            }
          : {
              level: "attention",
              title: "Upgrades wait less than a day",
              text: "Owners have little time to see and reject a release before it installs.",
            },
      )
    }
  }
  return checks
}
