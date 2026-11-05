# 0016: Each account has a name of its own

Status: accepted, 2026-11-04. Behrad chose unique names ("Go with your pick!",
thread "Try the guard on B3Wallet locally"), over names shown with their
chain and over keeping repeated names.

**Decision: no two listed accounts of a wallet have the same label. Labels
are compared as candid-profile's names rule compares names (spec section
5.3, D-045): Unicode NFKC, lowercase, runs of spaces made one, ends trimmed.
`add_account` and the new `rename_account` refuse a label another listed
account has. The accounts the wallet makes itself are "ICP wallet",
"Bitcoin wallet" and "Ethereum wallet". An upgrade renames the ones it made
as "Main" to those names. Owners rename an account without approval, and
the rename is in the audit log (`AccountRenamed`).**

Until now the wallet named every account it made "Main" and never checked
for repeats, and nobody could change a label. So "send from Main" named
three accounts. A name is how a person and an agent say which account
they mean: the MCP server's `wallet_propose_transfer` takes a label, and
the guard reads labels in the owner's words
([decision 0012](0012-guard-in-wallet.md)). Each had to refuse a
repeated name or narrow it by the asset. With unique names, a name means
one account.

- **Same name, same rule.** The wallet compares labels exactly as the
  names rule matches them, so a label the wallet accepts is one the tools
  can always resolve. A fullwidth "Ｓavings" is "Savings", and "BTC
  Savings" is " btc  savings ". Accents are not removed.
- **Contacts.** A contact's name ([decision 0015](0015-contacts.md)) is
  compared by the same key: an account cannot take a contact's name, and a
  contact cannot take an account's (`contacts.rs` `check_label_free`).
- **The wallet's own accounts.** Each is named for its chain. If an owner
  already took a name, the wallet takes the next free one ("Bitcoin
  wallet 2").
- **The upgrade.** It renames only accounts the wallet made (no `added_by`)
  that are still called "Main". An account an owner added, even one called
  "Main", keeps its name, and so does one an owner renamed: `rename_account`
  records who renamed it (`renamed_by`), so a later upgrade leaves even an
  owner's chosen "Main" alone. Labels that already repeat between owners'
  accounts stay until an owner renames one, and the tools keep refusing
  such a name as ambiguous.
- **Why no approval.** A rename moves nothing. Every screen shows the
  address beside the name ([b3wallet-new PR #37](https://github.com/b3hr4d/b3wallet-new/pull/37)),
  and a name is read once, when a call is prepared, so a rename never
  changes a transfer that is already proposed.

Checked:

- `canisters/wallet/src/accounts.rs` tests: the name key (NFKC, case,
  spaces), a refused repeat, a merged account's name being free, renaming
  to one's own name in another case, the default names, and which accounts
  an upgrade renames.
- PocketIC (`tests/integration/tests/wallet.rs`,
  `account_names_are_unique_and_owners_rename_them`): a repeated name is
  refused on add and on rename, a rename is audited, an old name is free
  again, and only owners may rename.

Open:

- **Member labels** may still repeat. A member chooses their own label when
  asking to join, so the same rule would refuse some requests to join.
