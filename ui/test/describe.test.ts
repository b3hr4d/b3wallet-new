import assert from "node:assert/strict"
import { test } from "node:test"
import {
  actionDetails,
  addressGroups,
  actionText,
  auditText,
  canRun,
  isJoinRequest,
  joinRequestText,
  receiveNote,
  shortText,
  sourceOf,
  stateText,
  waitsFor,
  walletErrorText,
  whoText,
} from "../src/logic/describe.ts"
import type { Account, Action, Member, Pending } from "../src/generated/wallet.ts"
import { agoText, dateOf, durationText } from "../src/logic/time.ts"

const owner = "fvski-cdv2h-g6ldz-eacds-4wfcf-px5sn-yfwv7-bwtxb-wvytu-cm3qa-sae" as never
const device = "gxhcg-wz6ja-zbwh7-s6ha4-nnlhc-ya5hh-2dv23-dwksw-ujozj-ymkcv-tae" as never
const SECOND = 1_000_000_000n
const NOW = 1_800_000_000n * SECOND

const main: Account = {
  id: 1,
  kind: { tag: "InternetComputer" },
  label: "Main",
  address: "efadq-gl777-77774-aaaba-cai",
  icp_account_id: null,
  index: 0,
  holder: { tag: "Wallet" },
}

const transfer: Pending = {
  id: 3n,
  action: {
    tag: "Transfer",
    value: { from: 1, asset: null, to: owner, amount: 150_000_000n, fee: null, memo: null },
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
  const coins: Account = { ...main, kind: { tag: "Bitcoin", value: { tag: "Mainnet" } } }
  const action: Action = {
    tag: "Transfer",
    value: {
      from: 1,
      asset: null,
      to: "bc1qexample",
      amount: 10_000n,
      fee: 500_000n,
      memo: null,
    },
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

test("an EVM transfer names the most it pays in fees, and its transaction once sent", () => {
  const eth = { symbol: "ETH on Base", decimals: 18, fee: null }
  const evm: Account = { ...main, kind: { tag: "Evm" } }
  const action: Action = {
    tag: "Transfer",
    value: {
      from: 1,
      asset: { tag: "Evm", value: 8_453n },
      to: "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
      amount: 10n ** 17n,
      fee: 21_000n * 10n ** 9n,
      memo: null,
    },
  }
  assert.equal(
    actionText(action, evm, eth),
    "Send 0.1 ETH on Base from “Main” to 0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed, paying at most 0.000021 ETH on Base in fees",
  )
  const hash = `0x${"ab".repeat(32)}`
  const sent: Pending = {
    ...transfer,
    action,
    state: {
      tag: "Executed",
      value: {
        tag: "Sent",
        value: { tx_id: hash, at_ns: 0n, chain: { tag: "Evm", value: 8_453n }, account: 1 },
      },
    },
  }
  assert.equal(stateText(sent, NOW), `Sent: transaction ${hash} on Base.`)
  assert.equal(
    auditText({ tag: "AssetTracked", value: { tag: "Evm", value: 8_453n } }, []),
    "tracked Base",
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
    value: {
      from: 1,
      asset: null,
      to: owner,
      amount: 1n,
      fee: null,
      memo: new Uint8Array([0x0a, 0x0b]),
    },
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

test("a time in the last week reads as how long ago it was", () => {
  const minute = 60n * SECOND
  assert.equal(agoText(NOW, NOW), "just now")
  assert.equal(agoText(NOW - 59n * SECOND, NOW), "just now")
  assert.equal(agoText(NOW + minute, NOW), "just now")
  assert.equal(agoText(NOW - minute, NOW), "1 minute ago")
  assert.equal(agoText(NOW - 150n * minute, NOW), "2 hours ago")
  assert.equal(agoText(NOW - 6n * 1_440n * minute, NOW), "6 days ago")
  const weekAgo = NOW - 7n * 1_440n * minute
  assert.equal(agoText(weekAgo, NOW), new Date(Number(weekAgo / 1_000_000n)).toLocaleDateString())
})

const me: Member = {
  id: owner,
  role: { tag: "Owner" },
  label: "Laptop",
  added_at_ns: NOW,
  added_by: null,
}

test("an action waits for a member who may approve or run it", () => {
  const waiting: Pending = {
    ...transfer,
    approvals: [device],
    quorum: 2,
    state: { tag: "Waiting" },
  }
  assert.equal(waitsFor(waiting, me, NOW), true)
  assert.equal(waitsFor({ ...waiting, approvals: [device, owner] }, me, NOW), false)
  assert.equal(waitsFor(transfer, me, NOW), true)
  assert.equal(
    waitsFor({ ...transfer, executable_after_ns: NOW + 3_600n * SECOND }, me, NOW),
    false,
  )
  assert.equal(waitsFor({ ...transfer, state: { tag: "Expired" } }, me, NOW), false)
  for (const tag of ["Viewer", "Proposer"] as const) {
    const reader: Member = { ...me, role: { tag } }
    assert.equal(waitsFor(waiting, reader, NOW), false, tag)
    assert.equal(waitsFor(transfer, reader, NOW), false, tag)
  }
})

test("a long id is cut to its ends", () => {
  assert.equal(shortText("uxrrr-q7777-77774-qaaaq-cai"), "uxrrr-…-cai")
  assert.equal(shortText("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq", 8, 6), "bc1qar0s…wf5mdq")
  assert.equal(shortText("aaaaa-aa"), "aaaaa-aa")
})

test("the audit log names who did what", () => {
  const phone: Member = { ...me, id: device, role: { tag: "Approver" }, label: "Phone" }
  assert.equal(whoText(owner, me, [me, phone]), "You")
  assert.equal(whoText(device, me, [me, phone]), "Phone")
  assert.equal(whoText(device, me, undefined), "gxhcg-…-tae")
  assert.equal(whoText(null, me, [me]), "The wallet")
  assert.equal(auditText({ tag: "Proposed", value: 4n }, undefined), "proposed action 4")
  assert.equal(auditText({ tag: "AccountAdded", value: 1 }, [main]), "added the account “Main”")
  assert.equal(auditText({ tag: "AccountAdded", value: 2 }, [main]), "added account 2")
  const ledger = "mxzaz-hqaaa-aaaar-qaada-cai" as never
  assert.equal(
    auditText({ tag: "AssetTracked", value: { tag: "Icrc", value: ledger } }, []),
    `tracked the ICRC-1 token of ledger ${ledger}`,
  )
  assert.equal(
    auditText({ tag: "AssetUntracked", value: { tag: "Icrc", value: ledger } }, []),
    `stopped tracking the ICRC-1 token of ledger ${ledger}`,
  )
  assert.equal(
    auditText({ tag: "UpgradeInstalled", value: "0.2.0" }, []),
    "installed version 0.2.0",
  )
  assert.equal(
    auditText({ tag: "CyclesLow", value: 420_000_000_000n }, []),
    "ran low on cycles, with 0.42T cycles left",
  )
})

test("an address reads in groups that join back into it", () => {
  const bitcoin = "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh"
  assert.deepEqual(addressGroups(bitcoin).slice(0, 3), ["bc1q", "xy2k", "gdyg"])
  assert.equal(addressGroups(bitcoin).join(""), bitcoin)
  assert.deepEqual(addressGroups("uxrrr-q7777-77774-qaaaq-cai"), ["uxrrr-q7777-77774-qaaaq-cai"])
  assert.deepEqual(addressGroups(""), [])
})

test("a sender learns what an address counts, and in which form", () => {
  const ic = { tag: "InternetComputer" } as const
  assert.match(receiveNote(ic), /^Send ICP or an ICRC-1 token the wallet tracks here/)
  assert.match(receiveNote(ic), /once an owner tracks its ledger/)
  assert.match(receiveNote(ic, "identifier"), /^For ICP only, from an exchange/)
  assert.match(receiveNote({ tag: "Bitcoin", value: { tag: "Mainnet" } }), /Bitcoin network/)
  assert.match(
    receiveNote({ tag: "Bitcoin", value: { tag: "Regtest" } }),
    /regtest bitcoin here, never mainnet/,
  )
  assert.match(
    receiveNote({ tag: "Evm" }),
    /^Send only ETH here, on an EVM chain the wallet tracks/,
  )
  assert.match(receiveNote({ tag: "Evm" }), /no ERC-20 token/)
})
