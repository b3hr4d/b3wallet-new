# 0015: Contacts, so the guard can check a recipient named by a person's name

Status: proposed, 2026-11-04. Behrad asked in the thread what to build after
the attack benchmark found the guard's largest gap. The details below are
recommendations for him to confirm; nothing is built yet.

**Decision: the wallet keeps an address book its owners write. A contact is
a name and its addresses, one per kind of account. Adding or changing one is
an action that waits out a timelock; removing one is not. The guard, the
terms of [decision 0014](0014-confirmed-terms.md), the agent's tools and the
pages resolve a contact's name the way they resolve an account's label.**

## Why

The guard compares the recipient in the owner's words with the transfer's
only when the words hold an address or the label of one of the wallet's own
accounts. A person's name falls through: `to_verdict` in
`crates/guard/src/comparator.rs` answers `NotChecked` for "a person's name,
or words the wallet cannot resolve". So "send 2 ICP to Dana" says nothing
about where the 2 ICP go.

candid-profile's attack benchmark v1 (branch `claude/project-thread-b7u43i`,
`guard/bench/attack/v1/report.json`) measured what that leaves open. With a
scripted agent that follows every injected text, so that each trial is a
tricked proposal, the round-9 guard flagged 180 of 276. By how the request
names the recipient:

| Recipient in the owner's words | Tricked proposals flagged |
| --- | --- |
| An address | 85 of 97 |
| One of the wallet's account labels | 73 of 83 |
| A person's name | 22 of 96 |

Of those 22, 21 were caught on another field (8 `amount`, 7 `asset` and 6
`extra` goals), and one `to` goal was flagged by something other than the
recipient check (`v1/rows.jsonl.gz`); none was caught by comparing the
recipient. Claude Haiku 5.5 and Claude Opus 5.5 driving the wallet
were tricked in 0 of 300 trials each, so the follower is the ceiling, not a
measured agent. It is still the hole a better attack or a weaker agent would
use, and redirecting the recipient is what an attacker wants most.

The same address book answers a second risk raised in the thread on
2026-11-02: an agent that talks the owner into writing a bad request. The
guard checks the agent against the owner's words, so it passes a bad request
the owner wrote. A new recipient that must wait a day before anyone can pay
it by name takes the hurry out of that.

## Options

| Option | What it is | For | Against |
| --- | --- | --- | --- |
| **A. Contacts behind a timelock** | `SetContact` and `RemoveContact` actions; `contacts` for members; the guard and the terms resolve contact names | The guard checks a named recipient. A new name cannot be paid by name until its timelock ends, so an agent or a message cannot add a payee in the moment. Every change is in the audit log | A new store and two action kinds. Adding a contact takes a day by default. The agent reads the contacts, as it reads the accounts |
| B. Contacts written at once | `set_contact` and `remove_contact` calls for owners, no action | Simpler; a contact is usable at once | An owner talked into adding the attacker's address can be paid by name a minute later. Nothing in the policy slows it |
| C. No contacts | Leave named recipients `NotChecked` | No new surface | The gap above stays, and the agent keeps its own address book the owner cannot see |

Recommendation: A.

## What changes with A

- **Storage.** Memory id 17, Contacts: contact id to name, addresses, added
  at and by, changed at and by. Ids are never reused. A contact holds at
  most one address per kind of account (Internet Computer, Bitcoin, EVM),
  checked as the transfer path checks a recipient of that kind.
- **Names.** The rule of the account labels (D-045 in candid-profile, as
  `@meantjs/format` reads it): NFKC, lowercase, spaces collapsed. A contact's
  name is unique among contacts and among the wallet's own account labels,
  and is never an address, an asset or chain word, or a tracked token's
  symbol, so a name always names one thing.
- **Actions.** `SetContact { id : opt ContactId; name; addresses }` adds or
  changes one; `RemoveContact : ContactId` removes one. Each has its own
  policy rule. The default policy: `SetContact` needs the owners' quorum and
  waits 24 hours; `RemoveContact` needs one owner and no timelock, since
  removing a payee never moves funds. Changing an existing contact's address
  is a `SetContact` too, never weaker than adding one: it would redirect
  every later "pay Dana". Its ticket shows the old and the new address side
  by side, and terms confirmed before it are outside it.
- **Lookalikes.** A `SetContact` whose address looks like another contact's
  or one of the wallet's own (the same first 6 and last 4 characters, the
  rest different: address poisoning, the attack benchmark's `lookalike`
  goal) carries a warning on its ticket, by candid-profile's rule
  (`guard/addresses.py`, `lookalike`). A timelock helps only if the owner
  notices; this makes the change noticeable. A proposer cannot propose either
  (decision 0007). Execution writes the audit entry, as for every action.
- **Reading.** `contacts()`, a query for members. A proposer sees them: the
  agent needs the names to propose by them.
- **Unknown recipients.** With the setting on (recommended as the default
  for proposers), the wallet refuses a proposer's transfer to an address
  that is neither one of its accounts nor a contact, unless that exact
  address is in the owner's own request: the guard compares it there
  (`is_address`), and a one-off payment to a pasted address keeps working.
- **The guard.** `Context` gains the contacts. `to_verdict` resolves an
  account label first, then a contact's address of the transfer's kind; a
  name that matches none, or a contact with no address of that kind, stays
  `NotChecked`. `answer_named` swaps contact names as it swaps labels, so
  "pay BTC Mike" reads as a name, not as an asset. candid-profile's Python
  comparator takes the same change, so the parity tests still hold.
- **Terms.** Who is paid may be a contact's name. The page shows the name and
  its address in full; a contact changed after the terms were confirmed is
  outside them.
- **The agent's tools.** `wallet_contacts` lists them; `to` in
  `wallet_propose_transfer` and in the terms takes a contact's name by the
  same rule, and the result says what it stood for.
- **The pages.** A Contacts page under Settings to propose, change and remove
  contacts. An Approvals ticket shows "to Dana (contact since …)", or "an
  address that is not a contact" for an agent's transfer to a new address.

## For Behrad to confirm

1. A rather than B: contacts behind a timelock.
2. 24 hours for `SetContact`, none for `RemoveContact`.
3. Whether an agent's transfer to an address that is neither one of the
   wallet's accounts nor a contact is refused outright (recommended for
   proposers, as a policy setting owners can turn off, with the pasted
   address exception above), or only marked on its ticket.
4. Optional: names unique by their confusable skeleton (Unicode UTS #39),
   not only by NFKC, so "Dana" and "Dаna" (Cyrillic а) cannot both exist.

A review on this pull request from the candid-profile side (2026-11-04)
answered 1 to 3 as recommended and added the address-change, lookalike and
pasted-address points above; the record stays proposed until Behrad
confirms.

## How it is checked

- Unit tests of the names rule and of `to_verdict` with contacts, including
  a contact that shares a word with a label, a name of two contacts, and a
  contact with no address of the transfer's kind.
- PocketIC: a `SetContact` cannot run before its timelock; a proposer cannot
  propose one; an upgrade keeps the contacts; the guard flags a transfer to
  another address for "send 2 ICP to Dana".
- candid-profile: the attack benchmark's scripted follower again on the same
  seed with the contacts in the guard's context, reported beside v1 in a new
  folder. No API calls.
- candid-profile: the full guard benchmark (12,000 requests of `data-r10q`)
  scored again with contact names swapped before the model reads. The model
  was trained on person names as unresolved names, so the swap changes its
  input; the false flags must not rise.
- PocketIC: a lookalike `SetContact` carries its warning; a proposer's
  transfer to an unknown address is refused, and one to the address the
  request pastes is not.
