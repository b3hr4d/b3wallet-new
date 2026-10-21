import assert from "node:assert/strict"
import { test } from "node:test"
import {
  actionText,
  canRun,
  isJoinRequest,
  joinRequestText,
  sourceOf,
  stateText,
} from "../src/describe.ts"
import type { Account, Pending } from "../src/generated/wallet.ts"
import { dateOf } from "../src/time.ts"

const owner = "fvski-cdv2h-g6ldz-eacds-4wfcf-px5sn-yfwv7-bwtxb-wvytu-cm3qa-sae" as never
const device = "gxhcg-wz6ja-zbwh7-s6ha4-nnlhc-ya5hh-2dv23-dwksw-ujozj-ymkcv-tae" as never
const SECOND = 1_000_000_000n
const NOW = 1_800_000_000n * SECOND

const main: Account = {
  id: 1,
  chain: { tag: "Icp" },
  label: "Main",
  address: "efadq-gl777-77774-aaaba-cai",
  index: 0,
  holder: { tag: "Wallet" },
}

const transfer: Pending = {
  id: 3n,
  action: {
    tag: "Transfer",
    value: { from: 1, to: owner, amount: 150_000_000n, fee: null, memo: null },
  },
  proposed_by: owner,
  proposed_at_ns: NOW,
  approvals: [owner],
  quorum: 1,
  executable_after_ns: NOW,
  expires_at_ns: NOW + 7n * 86_400n * SECOND,
  state: { tag: "Ready" },
}

test("a transfer names its amount and account once they are known", () => {
  const icp = { symbol: "ICP", decimals: 8, fee: 10_000n }
  assert.equal(actionText(transfer.action, main, icp), `Send 1.5 ICP from “Main” to ${owner}`)
  assert.equal(
    actionText(transfer.action, undefined, undefined),
    `Send 150000000 base units from account 1 to ${owner}`,
  )
})

test("a transfer's source is the account it names", () => {
  const cycles: Account = { ...main, id: 2, label: "Cycles" }
  assert.equal(sourceOf(transfer.action, [cycles, main]), main)
  assert.equal(sourceOf(transfer.action, [cycles]), undefined)
  assert.equal(sourceOf(transfer.action, undefined), undefined)
  assert.equal(sourceOf({ tag: "RemoveMember", value: owner }, [main]), undefined)
})

test("a join request is an AddMember of its own proposer", () => {
  const join: Pending = {
    ...transfer,
    action: { tag: "AddMember", value: { id: device, role: { tag: "Viewer" }, label: "Phone" } },
    proposed_by: device,
    approvals: [],
    state: { tag: "Waiting" },
  }
  assert.equal(isJoinRequest(join), true)
  assert.equal(joinRequestText(join), `${device} asks to join as a viewer, “Phone”`)
  assert.equal(stateText(join, NOW), "Waiting for approvals: 0 of 1.")
  assert.equal(isJoinRequest({ ...join, proposed_by: owner }), false)
  assert.equal(actionText(join.action, undefined, undefined), `Add ${device} as a viewer, “Phone”`)
})

test("an action runs once its timelock ends, within a minute's difference of clocks", () => {
  assert.equal(canRun(transfer, NOW), true)
  assert.equal(stateText(transfer, NOW), "Approved and ready to run.")
  const soon = { ...transfer, executable_after_ns: NOW + 30n * SECOND }
  assert.equal(canRun(soon, NOW), true)
  const later = { ...transfer, executable_after_ns: NOW + 86_400n * SECOND }
  assert.equal(canRun(later, NOW), false)
  assert.equal(
    stateText(later, NOW),
    `Approved. It can run after ${dateOf(later.executable_after_ns)}.`,
  )
  assert.equal(canRun({ ...transfer, state: { tag: "Waiting" } }, NOW), false)
})

test("an action that ended says how", () => {
  const sent = (chain: { tag: "Icp" } | { tag: "Bitcoin"; value: { tag: "Regtest" } }) =>
    stateText(
      {
        ...transfer,
        state: {
          tag: "Executed",
          value: { tag: "Sent", value: { account: 1, chain, tx_id: "6", at_ns: NOW } },
        },
      },
      NOW,
    )
  assert.equal(sent({ tag: "Icp" }), "Sent: block 6 of the ledger.")
  assert.equal(sent({ tag: "Bitcoin", value: { tag: "Regtest" } }), "Sent: Bitcoin transaction 6.")
  assert.equal(
    stateText({ ...transfer, state: { tag: "Executed", value: { tag: "Done" } } }, NOW),
    "Done.",
  )
  assert.equal(
    stateText({ ...transfer, state: { tag: "Rejected", value: owner } }, NOW),
    `Rejected by ${owner}.`,
  )
  assert.equal(stateText({ ...transfer, state: { tag: "Expired" } }, NOW), "Expired.")
  assert.equal(
    stateText({ ...transfer, state: { tag: "Failed", value: "no funds" } }, NOW),
    "Failed: no funds",
  )
})
