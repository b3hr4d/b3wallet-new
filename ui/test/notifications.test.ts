import assert from "node:assert/strict"
import { test } from "node:test"
import type { AuditEntry } from "../src/generated/wallet.ts"
import { newestOf, seenKey, unreadOf } from "../src/logic/notifications.ts"

const me = "fvski-cdv2h-g6ldz-eacds-4wfcf-px5sn-yfwv7-bwtxb-wvytu-cm3qa-sae"
const phone = "gxhcg-wz6ja-zbwh7-s6ha4-nnlhc-ya5hh-2dv23-dwksw-ujozj-ymkcv-tae"
const entry = (seq: bigint, by: string | null): AuditEntry => ({
  seq,
  at_ns: seq,
  by: by as never,
  event: { tag: "Approved", value: seq },
})
// Newest first, as the wallet answers.
const log = [entry(9n, phone), entry(8n, me), entry(7n, null), entry(6n, phone), entry(5n, phone)]

test("what is new is what others and the wallet did since you last looked", () => {
  assert.deepEqual(
    unreadOf(log, 6n, me).map((e) => e.seq),
    [9n, 7n],
  )
  assert.deepEqual(unreadOf(log, 9n, me), [])
  assert.deepEqual(
    unreadOf(log, 0n, me).map((e) => e.seq),
    [9n, 7n, 6n, 5n],
  )
})

test("on a first visit nothing is new", () => {
  assert.deepEqual(unreadOf(log, undefined, me), [])
})

test("the newest entry is the one to remember", () => {
  assert.equal(newestOf(log), 9n)
  assert.equal(newestOf([]), 0n)
})

test("each wallet and principal is remembered apart", () => {
  assert.notEqual(
    seenKey("uxrrr-q7777-77774-qaaaq-cai", me),
    seenKey("uxrrr-q7777-77774-qaaaq-cai", phone),
  )
  assert.notEqual(
    seenKey("uxrrr-q7777-77774-qaaaq-cai", me),
    seenKey("aovwi-4maaa-aaaaa-qaagq-cai", me),
  )
})
