import assert from "node:assert/strict"
import { test } from "node:test"
import { checkupOf } from "../src/logic/checkup.ts"
import type { Member, Policy, PolicyRule } from "../src/generated/wallet.ts"

const DAY = 86_400_000_000_000n
const wallet = "uxrrr-q7777-77774-qaaaq-cai"
const member = (id: string, role: Member["role"]["tag"]): Member => ({
  id: id as never,
  role: { tag: role },
  label: id,
  added_at_ns: 0n,
  added_by: null,
})
const rule = (
  kind: PolicyRule["kind"]["tag"],
  quorum: number,
  timelock_ns: bigint,
): PolicyRule => ({
  kind: { tag: kind },
  quorum,
  timelock_ns,
  daily_limits: [],
})
/** The wallet's default policy: one approval everywhere. */
const defaults: Policy = {
  rules: [rule("Transfer", 1, 0n), rule("AddMember", 1, 0n), rule("Upgrade", 1, 7n * DAY)],
}

const levels = (checks: ReturnType<typeof checkupOf>) =>
  Object.fromEntries(checks.map((check) => [check.title, check.level]))

test("a new wallet on the default policy is told what is missing", () => {
  const checks = checkupOf({
    wallet,
    members: [member("laptop", "Owner")],
    policy: defaults,
    controllers: [wallet],
  })
  assert.deepEqual(levels(checks), {
    "Its own only controller": "ok",
    "One owner": "attention",
    "One approval sends any amount": "attention",
    "Adding a member needs no wait": "attention",
    "Upgrades wait": "ok",
  })
  assert.equal(checks.find((check) => check.title === "One owner")?.href, "/members")
})

test("a wallet set up with care passes", () => {
  const careful: Policy = {
    rules: [
      {
        ...rule("Transfer", 2, 0n),
        daily_limits: [{ chain: { tag: "Icp" }, amount: 10_000_000_000n }],
      },
      rule("AddMember", 2, DAY),
      rule("Upgrade", 2, 7n * DAY),
    ],
  }
  const checks = checkupOf({
    wallet,
    members: [member("laptop", "Owner"), member("phone", "Owner")],
    policy: careful,
    controllers: [wallet],
  })
  assert.ok(checks.every((check) => check.level === "ok"))
  assert.equal(
    checks.find((check) => check.title === "Adding a member is guarded")?.text,
    "2 approvals, then a wait of 1 day.",
  )
  const noWait = checkupOf({
    wallet,
    members: undefined,
    policy: { rules: [rule("AddMember", 2, 0n)] },
    controllers: undefined,
  })
  assert.equal(noWait[0]?.text, "2 approvals, and no wait.")
  assert.match(
    checks.find((check) => check.title === "Transfers need 2 approvals")?.text ?? "",
    /100 ICP/,
  )
})

test("another controller is a problem", () => {
  const [control] = checkupOf({
    wallet,
    members: undefined,
    policy: undefined,
    controllers: [wallet, "aaaaa-aa"],
  })
  assert.equal(control?.level, "problem")
  assert.match(control?.text ?? "", /^aaaaa-aa can install any code/)
  const [none] = checkupOf({ wallet, members: undefined, policy: undefined, controllers: [] })
  assert.equal(none?.level, "problem")
  // Before anything is read, nothing is claimed.
  assert.deepEqual(
    checkupOf({ wallet, members: undefined, policy: undefined, controllers: undefined }),
    [],
  )
})
