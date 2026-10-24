import { formatUnits } from "@ic-reactor/core"
import type { Member, Pending, Status } from "../generated/wallet.ts"
import { actionText, isJoinRequest, waitsFor } from "./describe.ts"
import type { Path } from "./page.ts"
import { leftText } from "./time.ts"
import { LOW_CYCLES } from "./tokens.ts"

/** All good; something needs you or is worth watching; something is wrong. */
export type Level = "ok" | "attention" | "problem"

/** What a check is about, so a page can leave out one it shows otherwise. */
export type CheckKey = "status" | "cycles" | "version" | "waiting" | "changes" | "session"

/** One thing the dot in the bar checks, said as a title and a line. */
export type Check = { key?: CheckKey; level: Level; title: string; text: string; href?: Path }

/** How the wallet is doing: the worst level of its checks, and that check's title. */
export type Health = { level: Level; summary: string; checks: Check[] }

const RANK: Record<Level, number> = { ok: 0, attention: 1, problem: 2 }

/** A session that ends within this is worth a word before it does. */
const SOON_NS = 10n * 60_000_000_000n

/**
 * The wallet's health, for the dot in the bar on every page, from what the
 * page reads anyway: its status (public), and for a member the open actions.
 * A failed read, a failed upgrade or an ended session is a problem; low
 * cycles, an upgrade under way, actions that wait for you, open changes that
 * are not transfers and a session about to end need attention.
 */
export function healthOf({
  status,
  statusFailure,
  me,
  pending,
  sessionEndsNs,
  nowNs,
}: {
  status: Status | undefined
  statusFailure: string | undefined
  me: Member | null | undefined
  pending: readonly Pending[] | undefined
  sessionEndsNs: bigint | undefined
  nowNs: bigint
}): Health {
  const checks: Check[] = []
  if (statusFailure !== undefined) {
    checks.push({
      key: "status",
      level: "problem",
      title: "The wallet does not answer",
      text: statusFailure,
    })
  }
  if (status !== undefined) {
    const cycles = `${formatUnits(status.cycles, 12, { maxFractionDigits: 2 })}T`
    checks.push(
      status.cycles < LOW_CYCLES
        ? {
            key: "cycles",
            level: "attention",
            title: "Low on cycles",
            text: `${cycles} left. An upgrade needs about 0.35T to spare, and anyone can top it up.`,
          }
        : {
            key: "cycles",
            level: "ok",
            title: "Cycles",
            text: `${cycles} to pay for its calls and storage.`,
          },
    )
    const upgrade = status.upgrade
    switch (upgrade.tag) {
      case "Idle":
        checks.push({
          key: "version",
          level: "ok",
          title: `Version ${status.version}`,
          text: "No upgrade under way.",
        })
        break
      case "Staging":
        checks.push({
          key: "version",
          level: "attention",
          title: `Staging version ${upgrade.value.version}`,
          text: `${upgrade.value.chunks_uploaded} of ${upgrade.value.chunks_total} chunks uploaded.`,
        })
        break
      case "Installing":
        checks.push({
          key: "version",
          level: "attention",
          title: `Installing version ${upgrade.value.version}`,
          text: "The wallet answers again once the new version runs.",
        })
        break
      case "Failed":
        checks.push({
          key: "version",
          level: "problem",
          title: `The upgrade to version ${upgrade.value.version} failed`,
          text: upgrade.value.reason,
        })
        break
    }
  }
  if (me && pending !== undefined) {
    const mine = pending.filter((action) => waitsFor(action, me, nowNs)).length
    checks.push(
      mine > 0
        ? {
            key: "waiting",
            level: "attention",
            title: `${mine} wait${mine === 1 ? "s" : ""} for you`,
            text: "Approve or run them on the Approvals page.",
            href: "/approvals",
          }
        : {
            key: "waiting",
            level: "ok",
            title: "Nothing waits for you",
            text: "No open action needs you.",
          },
    )
    const changes = pending.filter(
      (action) => action.action.tag !== "Transfer" && !isJoinRequest(action),
    )
    const [first] = changes
    if (first !== undefined) {
      const more = changes.length > 1 ? `, and ${changes.length - 1} more` : ""
      checks.push({
        key: "changes",
        level: "attention",
        title: `${changes.length} change${changes.length === 1 ? "" : "s"} open`,
        text: `${actionText(first.action, undefined, undefined)}${more}. If you did not expect it, an owner can reject it.`,
        href: "/approvals",
      })
    }
  }
  if (sessionEndsNs !== undefined) {
    const left = sessionEndsNs - nowNs
    checks.push(
      left <= 0n
        ? {
            key: "session",
            level: "problem",
            title: "Your session ended",
            text: "Sign in again to act.",
          }
        : {
            key: "session",
            level: left < SOON_NS ? "attention" : "ok",
            title: left < SOON_NS ? "Your session ends soon" : "Signed in",
            text: `Internet Identity session, ${leftText(sessionEndsNs, nowNs)}.`,
          },
    )
  }
  checks.sort((a, b) => RANK[b.level] - RANK[a.level])
  const [worst] = checks
  if (worst === undefined) return { level: "ok", summary: "Checking the wallet…", checks }
  return {
    level: worst.level,
    summary: worst.level === "ok" ? "All good" : worst.title,
    checks,
  }
}
