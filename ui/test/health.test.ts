import assert from "node:assert/strict"
import { test } from "node:test"
import type { Member, Pending, Status } from "../src/generated/wallet.ts"
import { healthOf } from "../src/logic/health.ts"
import { leftText } from "../src/logic/time.ts"

const owner = "fvski-cdv2h-g6ldz-eacds-4wfcf-px5sn-yfwv7-bwtxb-wvytu-cm3qa-sae" as never
const phone = "gxhcg-wz6ja-zbwh7-s6ha4-nnlhc-ya5hh-2dv23-dwksw-ujozj-ymkcv-tae" as never
const SECOND = 1_000_000_000n
const HOUR = 3_600n * SECOND
const NOW = 1_800_000_000n * SECOND

const status: Status = {
  version: "0.1.0",
  claimed: true,
  claim_expires_at_ns: null,
  cycles: 4_210_000_000_000n,
  upgrade: { tag: "Idle" },
  release_channel: "aovwi-4maaa-aaaaa-qaagq-cai" as never,
  bitcoin_network: { tag: "Mainnet" },
}
const me: Member = {
  id: owner,
  role: { tag: "Owner" },
  label: "Laptop",
  added_at_ns: 0n,
  added_by: null,
}
const transfer: Pending = {
  id: 3n,
  action: {
    tag: "Transfer",
    value: { from: 1, asset: null, to: phone, amount: 1n, fee: null, memo: null },
  },
  proposed_by: phone,
  proposed_at_ns: NOW,
  approvals: [],
  quorum: 2,
  executable_after_ns: 0n,
  expires_at_ns: NOW + 24n * HOUR,
  state: { tag: "Waiting" },
}
const healthy = {
  status,
  statusFailure: undefined,
  me,
  pending: [],
  sessionEndsNs: NOW + 8n * HOUR,
  nowNs: NOW,
}

test("a wallet with nothing to do is all good", () => {
  const health = healthOf(healthy)
  assert.equal(health.level, "ok")
  assert.equal(health.summary, "All good")
  assert.deepEqual(
    health.checks.map((check) => check.title),
    ["Cycles", "Version 0.1.0", "Nothing waits for you", "Signed in"],
  )
  assert.equal(health.checks.at(-1)?.text, "Internet Identity session, 8 hours left.")
})

test("what needs you comes first and names the dot", () => {
  const health = healthOf({ ...healthy, pending: [transfer, { ...transfer, id: 4n }] })
  assert.equal(health.level, "attention")
  assert.equal(health.summary, "2 wait for you")
  assert.equal(health.checks[0]?.href, "/approvals")

  const low = healthOf({ ...healthy, status: { ...status, cycles: 620_000_000_000n } })
  assert.equal(low.level, "attention")
  assert.equal(low.summary, "Low on cycles")
  assert.match(low.checks[0]?.text ?? "", /^0\.62T left/)
})

test("an open change that is not a transfer is worth a word, a join request is not", () => {
  const upgrade: Pending = {
    ...transfer,
    id: 9n,
    proposed_by: owner,
    approvals: [owner],
    action: { tag: "Upgrade", value: { version: "0.2.0", wasm_sha256: new Uint8Array(32) } },
    state: { tag: "Ready" },
    executable_after_ns: NOW + 100n * HOUR,
  }
  const join: Pending = {
    ...transfer,
    id: 10n,
    action: { tag: "AddMember", value: { id: phone, role: { tag: "Viewer" }, label: "Phone" } },
  }
  const health = healthOf({ ...healthy, pending: [upgrade, join] })
  const change = health.checks.find((check) => check.title === "1 change open")
  assert.match(
    change?.text ?? "",
    /^Upgrade the wallet to version 0\.2\.0\. If you did not expect it/,
  )
})

test("a failed upgrade, a wallet that does not answer or an ended session is a problem", () => {
  const failed = healthOf({
    ...healthy,
    pending: [transfer],
    status: {
      ...status,
      upgrade: { tag: "Failed", value: { version: "0.2.0", reason: "trap", at_ns: NOW } },
    },
  })
  assert.equal(failed.level, "problem")
  assert.equal(failed.summary, "The upgrade to version 0.2.0 failed")

  const silent = healthOf({
    ...healthy,
    status: undefined,
    statusFailure: "Refused.",
    me: undefined,
  })
  assert.equal(silent.level, "problem")
  assert.equal(silent.summary, "The wallet does not answer")

  assert.equal(healthOf({ ...healthy, sessionEndsNs: NOW }).level, "problem")
  assert.equal(healthOf({ ...healthy, sessionEndsNs: NOW + 5n * 60n * SECOND }).level, "attention")
})

test("a viewer is told nothing about approvals they cannot give", () => {
  const viewer: Member = { ...me, role: { tag: "Viewer" } }
  const health = healthOf({ ...healthy, me: viewer, pending: [transfer] })
  assert.equal(health.level, "ok")
  assert.equal(health.checks.find((check) => check.title === "Nothing waits for you")?.level, "ok")
})

test("time left reads in its largest unit", () => {
  assert.equal(leftText(NOW + 8n * HOUR + 59n * SECOND, NOW), "8 hours left")
  assert.equal(leftText(NOW + 60n * SECOND, NOW), "1 minute left")
  assert.equal(leftText(NOW + 30n * SECOND, NOW), "under a minute left")
  assert.equal(leftText(NOW, NOW), "ended")
})
