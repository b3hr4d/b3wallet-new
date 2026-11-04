import assert from "node:assert/strict"
import { test } from "node:test"
import type { Account, Action, Contact } from "../src/generated/wallet.ts"
import { contactChange, looksLike, recipientText } from "../src/logic/contacts.ts"

const DANA = "rrkah-fqaaa-aaaaa-aaaaq-cai"
const LEE = "ryjl3-tyaaa-aaaaa-aaaba-cai"
const BTC = "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq"
const EVM = "0x52908400098527886E0F7030069857D2E4169EE7"

const main: Account = {
  id: 1,
  kind: { tag: "InternetComputer" },
  label: "Main",
  address: "efadq-gl777-77774-aaaba-cai",
  icp_account_id: null,
  index: 0,
  holder: { tag: "Wallet" },
}

function contact(id: number, name: string, addresses: Contact["addresses"]): Contact {
  return {
    id,
    name,
    addresses,
    added_at_ns: 0n,
    added_by: DANA as never,
    changed_at_ns: null,
    changed_by: null,
  }
}

const dana = contact(1, "Dana", [
  { kind: { tag: "InternetComputer" }, address: DANA },
  { kind: { tag: "Bitcoin", value: { tag: "Mainnet" } }, address: BTC },
])

/** `address` with one character between its first 6 and last 4 changed. */
function poisoned(address: string): string {
  const at = 8
  const swap = address[at] === "a" ? "b" : "a"
  return address.slice(0, at) + swap + address.slice(at + 1)
}

test("an address looks like another with the same first 6 and last 4 characters", () => {
  assert.ok(looksLike(poisoned(DANA), DANA))
  assert.ok(looksLike(poisoned(BTC), BTC))
  assert.ok(!looksLike(DANA, DANA), "the address itself")
  assert.ok(!looksLike(EVM.toLowerCase(), EVM), "an EVM address, case aside, is the same")
  assert.ok(looksLike(poisoned(EVM.toLowerCase()), EVM))
  assert.ok(!looksLike(LEE, DANA), "the ends differ")
  assert.ok(!looksLike("aaaaa-aa", "aaaaa-aa1"), "too short to tell")
})

test("a change shows the old and the new address side by side, and warns of a look-alike", () => {
  const change: Action = {
    tag: "SetContact",
    value: {
      id: 1,
      name: "Dana",
      addresses: [{ kind: { tag: "InternetComputer" }, address: LEE }],
    },
  }
  const shown = contactChange(change, [dana], [main])
  assert.equal(shown?.contact, dana)
  assert.equal(shown?.name, undefined, "the name stays")
  assert.deepEqual(shown?.addresses, [
    { kind: { tag: "InternetComputer" }, before: DANA, after: LEE },
    { kind: { tag: "Bitcoin", value: { tag: "Mainnet" } }, before: BTC, after: undefined },
  ])
  assert.deepEqual(shown?.warnings, [])

  const lookalike = poisoned(DANA)
  const added: Action = {
    tag: "SetContact",
    value: {
      id: null,
      name: "Dana B",
      addresses: [
        { kind: { tag: "InternetComputer" }, address: lookalike },
        { kind: { tag: "Evm" }, address: poisoned(main.address) },
      ],
    },
  }
  const fresh = contactChange(added, [dana], [main])
  assert.deepEqual(fresh?.name, { before: undefined, after: "Dana B" })
  assert.equal(fresh?.warnings.length, 2)
  assert.match(fresh!.warnings[0]!, /looks like “Dana” rrkah-fqaaa/)
  assert.match(fresh!.warnings[1]!, /looks like the wallet's own “Main”/)
  // A contact's own address is not a look-alike of itself when it is changed.
  const same: Action = {
    tag: "SetContact",
    value: {
      id: 1,
      name: "Dana",
      addresses: [{ kind: { tag: "InternetComputer" }, address: poisoned(DANA) }],
    },
  }
  assert.deepEqual(contactChange(same, [dana], [])?.warnings, [])

  const removed = contactChange({ tag: "RemoveContact", value: 1 }, [dana], [main])
  assert.deepEqual(
    removed?.addresses.map((address) => address.before),
    [DANA, BTC],
  )
  assert.equal(contactChange({ tag: "RemoveMember", value: DANA as never }, [dana], []), undefined)
})

test("a transfer names its recipient as a contact, an own account, or neither", () => {
  const to = (address: string): Action => ({
    tag: "Transfer",
    value: { from: 1, asset: null, to: address, amount: 1n, fee: null, memo: null },
  })
  assert.equal(recipientText(to(DANA), main, [main], [dana]), "To Dana (contact).")
  assert.equal(
    recipientText(to(main.address), main, [main], [dana]),
    "To the wallet's own account “Main”.",
  )
  assert.equal(
    recipientText(to(LEE), main, [main], [dana]),
    "To an address that is not a contact and not one of the wallet's accounts.",
  )
  // Dana's Bitcoin address is not her address for an Internet Computer transfer.
  assert.equal(
    recipientText(to(BTC), main, [main], [dana]),
    "To an address that is not a contact and not one of the wallet's accounts.",
  )
  assert.equal(recipientText(to(DANA), main, undefined, [dana]), undefined)
})
