import assert from "node:assert/strict"
import { test } from "node:test"
import type { Member, Policy, Role } from "../src/generated/wallet.ts"
import { changeRefusal, labelRefusal } from "../src/logic/governance.ts"

const member = (id: string, role: Role["tag"]): Member => ({
  id: id as never,
  role: { tag: role },
  label: id,
  added_at_ns: 0n,
  added_by: null,
})
const policy = (quorum: number): Policy => ({
  rules: [
    { kind: { tag: "Transfer" }, quorum, timelock_ns: 0n, daily_limits: [] },
    { kind: { tag: "AddMember" }, quorum: 1, timelock_ns: 0n, daily_limits: [] },
  ],
})

test("the wallet keeps an owner", () => {
  const members = [member("laptop", "Owner"), member("phone", "Viewer")]
  assert.equal(
    changeRefusal(members, policy(1), { remove: "laptop" }),
    "The wallet must keep an owner.",
  )
  assert.equal(
    changeRefusal(members, policy(1), { id: "laptop", role: { tag: "Approver" } }),
    "The wallet must keep an owner.",
  )
  assert.equal(changeRefusal(members, policy(1), { remove: "phone" }), undefined)
  const two = [...members, member("tablet", "Owner")]
  assert.equal(changeRefusal(two, policy(1), { remove: "laptop" }), undefined)
})

test("the wallet keeps as many approvers as its largest quorum", () => {
  const members = [
    member("laptop", "Owner"),
    member("phone", "Approver"),
    member("agent", "Proposer"),
  ]
  assert.match(changeRefusal(members, policy(2), { remove: "phone" }) ?? "", /asks for 2 approvals/)
  assert.match(
    changeRefusal(members, policy(2), { id: "phone", role: { tag: "Proposer" } }) ?? "",
    /must keep 2 owners and approvers/,
  )
  assert.equal(changeRefusal(members, policy(2), { remove: "agent" }), undefined)
  assert.equal(changeRefusal(members, policy(1), { remove: "phone" }), undefined)
  // Before the policy is read, the quorum counts as one.
  assert.equal(changeRefusal(members, undefined, { remove: "phone" }), undefined)
})

test("a name is 1 to 64 bytes, as the wallet takes it", () => {
  assert.equal(labelRefusal("Laptop"), undefined)
  assert.equal(labelRefusal("  "), "Give it a name.")
  assert.equal(labelRefusal("a".repeat(64)), undefined)
  assert.match(labelRefusal("a".repeat(65)) ?? "", /at most 64 bytes/)
  assert.match(labelRefusal("é".repeat(33)) ?? "", /at most 64 bytes/)
})
