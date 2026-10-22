import assert from "node:assert/strict"
import { test } from "node:test"
import {
  actionDetails,
  actionText,
  canRun,
  isJoinRequest,
  joinRequestText,
  sourceOf,
  stateText,
  walletErrorText,
} from "../src/describe.ts"
import type { Account, Action, Pending } from "../src/generated/wallet.ts"
import { dateOf, durationText } from "../src/time.ts"

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

test("a transfer names a fee set in its proposal", () => {
  const btc = { symbol: "BTC", decimals: 8, fee: null }
  const coins: Account = { ...main, chain: { tag: "Bitcoin", value: { tag: "Mainnet" } } }
  const action: Action = {
    tag: "Transfer",
    value: { from: 1, to: "bc1qexample", amount: 10_000n, fee: 500_000n, memo: null },
  }
  assert.equal(
    actionText(action, coins, btc),
    "Send 0.0001 BTC from “Main” to bc1qexample, paying a fee of 0.005 BTC",
  )
  assert.equal(
    actionText(action, undefined, undefined),
    "Send 10000 base units from account 1 to bc1qexample, paying a fee of 500000 base units",
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

  // An agent asks to be a proposer, and the owner reads that before approving.
  const agent: Pending = {
    ...join,
    action: { tag: "AddMember", value: { id: device, role: { tag: "Proposer" }, label: "Claude" } },
  }
  assert.equal(joinRequestText(agent), `${device} asks to join as a proposer, “Claude”`)
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

test("a long wait reads in days down to seconds", () => {
  assert.equal(durationText(0n), "no time")
  assert.equal(durationText(1n), "under a second")
  assert.equal(durationText(7n * 86_400n * SECOND), "7 days")
  assert.equal(durationText(86_400n * SECOND + 3_600n * SECOND), "1 day and 1 hour")
  assert.equal(
    durationText(2n * 3_600n * SECOND + 90n * SECOND),
    "2 hours, 1 minute and 30 seconds",
  )
})

test("an action shows what it sets beyond its sentence", () => {
  const day = 86_400n * SECOND
  const policy: Action = {
    tag: "SetPolicy",
    value: {
      rules: [
        {
          kind: { tag: "Transfer" },
          quorum: 2,
          timelock_ns: 0n,
          daily_limits: [{ chain: { tag: "Icp" }, amount: 100_000_000n }],
        },
        { kind: { tag: "SetPolicy" }, quorum: 2, timelock_ns: 7n * day, daily_limits: [] },
      ],
    },
  }
  assert.deepEqual(actionDetails(policy), [
    "A transfer: 2 approvals and no wait.",
    "A transfer that keeps the last 24 hours' spending on ICP within 1 ICP needs only its proposer and no wait.",
    "Changing the policy: 2 approvals and a wait of 7 days.",
  ])
  const ledger = "mxzaz-hqaaa-aaaar-qaada-cai" as never
  const extension: Action = {
    tag: "InstallExtension",
    value: {
      canister: ledger,
      granted: [{ tag: "ReadAccounts" }, { tag: "SpendSubWallet", value: { tag: "Icp" } }],
      budgets: [
        { chain: { tag: "Bitcoin", value: { tag: "Mainnet" } }, per_day: 50_000n, total: null },
        { chain: { tag: "Icrc", value: ledger }, per_day: 7n, total: 70n },
      ],
    },
  }
  assert.deepEqual(actionDetails(extension), [
    "It may read the accounts and their balances.",
    "It may spend from its own sub-wallet on ICP.",
    "On Bitcoin, it may spend 0.0005 BTC a day.",
    `On ICRC-1 token of ledger ${ledger}, it may spend 7 base units a day and 70 base units in all.`,
  ])
  assert.deepEqual(
    actionDetails({
      tag: "InstallExtension",
      value: { canister: ledger, granted: [], budgets: [] },
    }),
    ["It is granted nothing."],
  )
  assert.deepEqual(
    actionDetails({
      tag: "Upgrade",
      value: { version: "0.2.0", wasm_sha256: new Uint8Array([0, 1, 0xab, 0xff]) },
    }),
    ["The new version's Wasm has the SHA-256 0001abff."],
  )
  const memo: Action = {
    tag: "Transfer",
    value: { from: 1, to: owner, amount: 1n, fee: null, memo: new Uint8Array([0x0a, 0x0b]) },
  }
  assert.deepEqual(actionDetails(memo), ["Memo: 0a0b."])
  assert.deepEqual(actionDetails(transfer.action), [])
  assert.deepEqual(actionDetails({ tag: "RemoveMember", value: owner }), [])
})

test("a wallet too short of cycles says what it has and what it needs", () => {
  assert.equal(
    walletErrorText({
      tag: "InsufficientCycles",
      value: { available: 290_850_696_536n, needed: 340_000_000_001n },
    }),
    "The wallet has 0.29T cycles to spend, and this needs 0.35T cycles. Send it cycles, then try again.",
  )
})
