import assert from "node:assert/strict"
import { createHash } from "node:crypto"
import { test } from "node:test"
import type { Principal } from "@candid-core/schema"
import type { Account, Action, Chain, RequestTerms, TermsDraft } from "../src/generated/wallet.ts"
import {
  checkTransfer,
  confirmedTerms,
  endOf,
  readDraft,
  termsWallet,
  transferGrant,
  type LedgerToken,
} from "../src/logic/terms.ts"
import { ICP_LEDGER } from "../src/logic/tokens.ts"

const WALLET = "uxrrr-q7777-77774-qaaaq-cai" as Principal
const FRIEND = "rrkah-fqaaa-aaaaa-aaaaq-cai"
const CKBTC = "mxzaz-hqaaa-aaaar-qaada-cai" as Principal
const SPOOF = "ryjl3-tyaaa-aaaaa-aaaca-cai"
/** 2026-10-31 09:00 UTC, when the agent wrote the terms. */
const DRAFTED = 1_793_437_200n * 1_000_000_000n
const HOUR = 3_600n * 1_000_000_000n

const icp: LedgerToken = { ledger: ICP_LEDGER, token: { symbol: "ICP", decimals: 8, fee: 10_000n } }
const ckbtc: LedgerToken = { ledger: CKBTC, token: { symbol: "ckBTC", decimals: 8, fee: 10n } }

function account(id: number, label: string, address: string, tag = "InternetComputer"): Account {
  return {
    id,
    kind: tag === "Evm" ? { tag: "Evm" } : { tag: "InternetComputer" },
    label,
    index: id,
    holder: { tag: "Wallet" },
    address,
    icp_account_id: null,
  }
}

const main = account(0, "Main", WALLET)
const savings = account(1, "Épargne", `${WALLET}-eiw7ojy.1`)
const accounts = [main, savings]

function terms(draft: Partial<TermsDraft>, version = 1): RequestTerms {
  return {
    draft: {
      token: "ICP",
      to: FRIEND,
      max_amount: "1.5",
      expires_by: "2026-11-30",
      ...draft,
    },
    version,
    drafted_by: FRIEND as Principal,
    drafted_at_ns: DRAFTED,
    confirmed: null,
    declined: null,
  }
}

function transfer(to: string, amount: bigint, asset: Chain | null = null): Action {
  return { tag: "Transfer", value: { from: 0, asset, to, amount, fee: null, memo: null } }
}

const sha256 = (text: string) => createHash("sha256").update(text, "utf8").digest("hex")

test("a symbol or a label that would not name one thing is left out", () => {
  const wallet = termsWallet(
    [
      icp,
      { ledger: SPOOF, token: { symbol: "icp", decimals: 8, fee: null } },
      ckbtc,
      {
        ledger: "ss2fx-dyaaa-aaaar-qacoq-cai",
        token: { symbol: "CK-BTC", decimals: 8, fee: null },
      },
      { ledger: "apia6-jaaaa-aaaar-qabma-cai", token: { symbol: "$$", decimals: 8, fee: null } },
      { ledger: "xevnm-gaaaa-aaaar-qafnq-cai", token: { symbol: "US‮DC", decimals: 6, fee: null } },
      icp,
    ],
    [
      main,
      savings,
      account(2, "main", "aaaaa-aa"),
      account(3, "--", "2vxsx-fae"),
      account(4, "Spare", "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed", "Evm"),
      account(5, "Odd", "not an account"),
    ],
    DRAFTED,
    0,
  )
  // ICP keeps its name; the ledger that also calls itself ICP, the two ckBTCs, a symbol of no letters and one with a
  // right-to-left override are named nowhere.
  assert.deepEqual(wallet.tokens, { ICP: { canister: ICP_LEDGER, decimals: 8 } })
  // Two accounts called Main and main name neither; only Internet Computer accounts are named.
  assert.deepEqual(wallet.apps, { Épargne: savings.address })
  // ICP's name stays ICP's even when its ledger could not be read, and so does the symbol its ledger reports.
  const unread = termsWallet(
    [{ ledger: SPOOF, token: { symbol: "ICP", decimals: 8, fee: null } }, ckbtc],
    [],
    DRAFTED,
    0,
  )
  assert.deepEqual(unread.tokens, { ckBTC: { canister: CKBTC, decimals: 8 } })
  const local = termsWallet(
    [
      { ledger: ICP_LEDGER, token: { symbol: "LICP", decimals: 8, fee: null } },
      { ledger: SPOOF, token: { symbol: "licp", decimals: 8, fee: null } },
    ],
    [],
    DRAFTED,
    0,
  )
  assert.deepEqual(local.tokens, { LICP: { canister: ICP_LEDGER, decimals: 8 } })
})

test("the agent's draft reads as the owner confirms it, as of when it was written", () => {
  const read = readDraft(terms({}), [icp, ckbtc], accounts, 0)
  assert.equal(read.error, undefined)
  assert.equal(
    read.confirmation?.text,
    "rrkah-fqaaa-aaaaa-aaaaq-cai may be sent at most 1.5 ICP, until the end of 30 November 2026 (UTC).",
  )
  // A length counts from the draft, whenever the owner reads it; the zone is the browser's then.
  const later = readDraft(
    terms({ expires_by: "2 hours", max_amount: "0.25 ICP" }),
    [icp],
    accounts,
    180,
  )
  assert.equal(
    later.confirmation?.text,
    "rrkah-fqaaa-aaaaa-aaaaq-cai may be sent at most 0.25 ICP, until 31 October 2026 at 14:00 (UTC+03:00).",
  )
  assert.equal(endOf(later.terms!), DRAFTED + 2n * HOUR)
  assert.equal(endOf(readDraft(terms({ expires_by: "never" }), [icp], accounts, 0).terms!), null)
  // One of the wallet's own accounts, by its label, in any case: shown by name and account.
  const own = readDraft(terms({ to: "épargne", token: "ckbtc" }), [icp, ckbtc], accounts, 0)
  assert.equal(
    own.confirmation?.text,
    `Épargne (${savings.address}) may be sent at most 1.5 ckBTC, until the end of 30 November 2026 (UTC).`,
  )
})

test("a draft that cannot be read says why, and nothing is guessed or rounded", () => {
  const why = (draft: Partial<TermsDraft>) => readDraft(terms(draft), [icp], accounts, 0).error
  assert.match(why({ token: "ICPP" }) ?? "", /knows no token named "ICPP"; it knows ICP/)
  assert.match(why({ max_amount: "1.123456789" }) ?? "", /^max_amount: /)
  assert.match(why({ to: "Brother" }) ?? "", /knows no app named "Brother"/)
  assert.match(why({ expires_by: "2026-10-30" }) ?? "", /before today, 2026-10-31/)
  assert.match(why({ expires_by: "next Monday" }) ?? "", /^expires_by must be a date/)
})

test("the page confirms terms by their JSON and its SHA-256, which the wallet checks", () => {
  const read = readDraft(terms({ to: "Épargne" }), [icp], accounts, 60)
  const confirmation = read.confirmation!
  assert.equal(confirmation.version, 1)
  // The id the library gives terms is the SHA-256 of their JSON (RFC 8785) as UTF-8, as the wallet computes it.
  assert.equal(confirmation.id, sha256(confirmation.terms))
  assert.equal(
    confirmation.terms,
    `{"amount":{"units":"150000000","unlimited":false},"decimals":8,"ends":{"at":"1796079599.999999999","day":"2026-11-30"},"name":"Épargne","symbol":"ICP","to":"${savings.address}","token":"${ICP_LEDGER}","utcOffsetMinutes":60,"written":{"expires_by":"2026-11-30","max_amount":"1.5","to":"Épargne","token":"ICP"}}`,
  )
  // canisters/wallet/src/terms.rs takes these exact bytes and this id.
  assert.equal(confirmation.id, "4b53ee47f52b083bdecb7180cbba52a39a1dd840a8e7e4d0649fe73600ae4325")
  const back = confirmedTerms({ ...confirmation, by: WALLET, at_ns: DRAFTED })
  assert.equal(back?.id, confirmation.id)
  assert.equal(back?.to, savings.address)
})

test("confirmed terms that do not hold together are not read", () => {
  const { confirmation } = readDraft(terms({}), [icp], accounts, 0)
  const record = { ...confirmation!, by: WALLET, at_ns: DRAFTED }
  const raised = record.terms.replace('"150000000"', '"950000000"')
  assert.equal(confirmedTerms({ ...record, terms: raised }), undefined)
  // The same terms with a space are other bytes: the wallet would take them with their own id, and the page refuses
  // them, since they are not the JSON the library writes.
  const spaced = record.terms.replace(",", ", ")
  assert.equal(confirmedTerms({ ...record, terms: spaced, id: sha256(spaced) }), undefined)
  const withId = record.terms.replace("{", `{"id":"${record.id}",`)
  assert.equal(confirmedTerms({ ...record, terms: withId, id: sha256(withId) }), undefined)
  for (const terms of ["", "null", "[]", '"terms"', "{}", "{"]) {
    assert.equal(confirmedTerms({ ...record, terms, id: sha256(terms) }), undefined, terms)
  }
  assert.ok(confirmedTerms(record))
})

test("a transfer is checked against the confirmed terms when it would run", () => {
  const { confirmation } = readDraft(terms({}), [icp], accounts, 0)
  const confirmed = confirmedTerms({ ...confirmation!, by: WALLET, at_ns: DRAFTED })!
  const end = endOf(confirmed)!
  const check = (action: Action, now = DRAFTED + HOUR) =>
    checkTransfer(confirmed, action, main, accounts, now)
  assert.deepEqual(check(transfer(FRIEND, 150_000_000n)), { fits: true, reasons: [] })
  assert.deepEqual(check(transfer(FRIEND, 1n)), { fits: true, reasons: [] })
  assert.deepEqual(check(transfer(FRIEND, 150_000_001n)).reasons, [
    "the request allows 1.50000001 ICP, the terms say at most 1.5 ICP",
  ])
  assert.deepEqual(check(transfer(savings.address, 100_000_000n)).reasons, [
    `the recipient is Épargne (${savings.address}), the terms say ${FRIEND}`,
  ])
  const ck = transfer(FRIEND, 1n, { tag: "Icrc", value: CKBTC })
  assert.deepEqual(check(ck).reasons, [`the request is for ${CKBTC}, the terms say ICP`])
  assert.deepEqual(check(transfer(FRIEND, 1n), end), { fits: true, reasons: [] })
  assert.deepEqual(check(transfer(FRIEND, 1n), end + 1n).reasons, [
    "the terms have ended: they said until the end of 30 November 2026 (UTC)",
  ])
  // Bitcoin, or an EVM chain's token: no terms describe them, so none cover them.
  const bitcoin = transfer("bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080", 1n, {
    tag: "Bitcoin",
    value: { tag: "Regtest" },
  })
  assert.equal(transferGrant(bitcoin, main), null)
  assert.equal(check(bitcoin).fits, false)
  // A recipient that is not an ICRC-1 account is refused, with why.
  assert.equal(check(transfer("not an account", 1n)).fits, false)
})

test("terms naming the wallet's own account by its label cover a transfer to its address", () => {
  const { confirmation } = readDraft(terms({ to: "Épargne" }), [icp], accounts, 0)
  const confirmed = confirmedTerms({ ...confirmation!, by: WALLET, at_ns: DRAFTED })!
  assert.deepEqual(
    checkTransfer(confirmed, transfer(savings.address, 150_000_000n), main, accounts, DRAFTED),
    {
      fits: true,
      reasons: [],
    },
  )
  assert.deepEqual(transferGrant(transfer(savings.address, 7n), main), {
    kind: "transfer",
    to: savings.address,
    token: ICP_LEDGER,
    amount: { units: "7", unlimited: false },
    ends: { now: true },
  })
})
