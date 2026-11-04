# 0015: Contacts, so the guard can check a recipient named by a person's name

Status: accepted, 2026-11-04. Behrad chose option A in the thread, with 24
hours for `SetContact`, no timelock for `RemoveContact`, and a proposer's
transfer refused when its recipient is not a contact, not one of the
wallet's own accounts and not written in the owner's request, as a policy
setting owners can turn off. The two reviews on pull request #36, from the
candid-profile work that ran the guard and attack benchmarks, are folded in
below.

**Decision: the wallet keeps an address book its owners write. A contact is
a name and its addresses, one per kind of account, and nothing else. Adding
or changing one is an action that waits out a timelock; removing one is
not. The guard, the terms of [decision 0014](0014-confirmed-terms.md), the
agent's tools and the pages resolve a contact's name the way they resolve an
account's label, when the name is read.**

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

21 of those 22 were caught on another field (8 `amount`, 7 `asset` and 6
`extra` goals). The other one, a `to` goal, was flagged by something other
than the recipient check (`v1/rows.jsonl.gz`). None was caught by comparing
the recipient. By goal, the follower's unflagged trials were 29 of 60 for a
look-alike address, 25 of 60 for another recipient, 19 of 60 for the amount,
19 of 60 for an extra transfer and 4 of 36 for the asset. Claude Haiku 5.5
and Claude Opus 5.5 driving the wallet were tricked in 0 of 300 trials each,
by attacks Gemma 4 E4B wrote after Gemini refused, so the follower is the
ceiling, not a measured agent. It is still the hole a better attack or a
weaker agent would use, and redirecting the recipient is what an attacker
wants most.

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

Chosen: A.

## What changes

- **Storage.** Memory id 17, Contacts: contact id to name, addresses, added
  at and by, changed at and by. Ids are never reused. A contact holds at
  most one address per kind of account (Internet Computer, Bitcoin, EVM),
  each checked as the transfer path checks a recipient of that kind.
- **A contact is a name and addresses, nothing else.** There is no notes
  field. Contact notes were one of the attack benchmark's three injection
  channels, with token descriptions and inbox messages, and the agent reads
  the contacts.
- **Names.** The accepted names rule, D-045 in candid-profile
  (`docs/decisions.md`; proposal `docs/proposals/w9-names.md`; vectors
  `spec/names-vectors.json`):
  - a name is normalized by NFKC, the default lowercase mapping, whitespace
    runs collapsed to one space, and trimmed;
  - it names a thing only when exactly one thing has it, otherwise it is
    `not_found` or `ambiguous`;
  - a name that is itself a value, such as an address, never matches;
  - the value is shown whenever a name is.

  Clashes are resolved when a name is read, not only when it is saved:
  saving still refuses a contact whose name is another contact's, an
  account's label or an address, but a label or token added later, or
  labels that already repeat, are caught where the name is read. Ambiguous
  means `NotChecked` for the guard, and the agent asks. The pages and the
  agent's tools use `normalizeName` and `resolveName` from
  `@meantjs/format` ([decision 0013](0013-meant-format.md)) over the
  accounts and the contacts together; the wallet's Rust code and the guard
  apply the same rule.
- **Actions.** `SetContact { id : opt ContactId; name; addresses }` adds a
  contact (no id) or changes one; `RemoveContact : ContactId` removes one.
  Each has its own policy rule. The default policy: `SetContact` takes one
  approval and waits 24 hours; `RemoveContact` takes one approval and no
  timelock, since removing a payee never moves funds. A wallet upgraded to
  this release gets the same two rules, with `SetContact`'s timelock no
  longer than its `SetPolicy` rule's. A proposer cannot propose either
  (decision 0007). Execution writes the audit entry, as for every action.
- **Changing an address is as strict as adding a contact.** It is a
  `SetContact`, under the same rule: it would redirect every later "pay
  Dana". Its ticket shows the old and the new address in full, side by side,
  and terms confirmed before it are outside it.
- **Look-alikes.** A `SetContact` ticket carries a warning when one of its
  addresses looks like another contact's or one of the wallet's own: the same
  first 6 and last 4 characters (an EVM address case aside), the rest
  different. That is address poisoning, candid-profile's `lookalike` rule
  (`guard/addresses.py`) and the goal the guard missed most often. A
  timelock helps only if the owner notices; this makes the change noticeable.
- **Reading.** `contacts()`, a query for members. A proposer sees them: the
  agent needs the names to propose by them.
- **Unknown recipients.** The policy gains `proposer_recipients`: `Known`
  (the default, also for a wallet whose stored policy has none) or `Any`.
  Under `Known` the wallet refuses a proposer's transfer whose recipient is
  not the address of one of its accounts, not an address of a contact of the
  transfer's kind, and not written in the owner's request it answers. "Written
  in the request" means one of the request's words is that address, as the
  comparator compares addresses: a one-off payment to a pasted address keeps
  working, and the guard checks it there. Owners turn it off with a
  `SetPolicy`, which waits 7 days by default.
- **The guard.** `Context` gains the contacts. `to_verdict` reads a name
  against the account labels and the contact names together: exactly one
  match is an account's address, or the contact's address of the transfer's
  kind; no match, two or more, or a contact with no address of that kind
  stays `NotChecked`. Contact names are swapped before the model reads as
  labels are only if the re-scored guard benchmark shows it does not raise
  the false flags (below); the model was trained on person names as
  unresolved names. candid-profile's Python comparator takes the same change,
  so the parity tests still hold.
- **Terms.** Who is paid may be a contact's name. The page shows the name and
  its address in full; a contact changed after the terms were confirmed is
  outside them.
- **The agent's tools.** `wallet_contacts` lists them; `to` in
  `wallet_propose_transfer` and in the terms takes a contact's name by the
  same rule, and the result says what it stood for.
- **The pages.** A Contacts card on the Settings page to propose, change and
  remove contacts. An Approvals ticket shows "to Dana (contact)", or "an
  address that is not a contact" for a transfer to an address that is neither
  a contact nor one of the wallet's own.

Not covered:

- **Confusables.** Names are unique by NFKC, not by their UTS #39 confusable
  skeleton, so "Dana" and "Dаna" (with a Cyrillic а) can both exist. The
  look-alike warning covers addresses, not names.
- **The warning is the page's.** Another client of an owner shows no
  look-alike warning, as with the terms check of decision 0014.

## How it is checked

- Unit tests of the names rule against candid-profile's
  `spec/names-vectors.json`, and of `to_verdict` with contacts, including a
  contact that shares a name with a label, a name of two contacts, a name
  that is an address, and a contact with no address of the transfer's kind.
- The parity tests: the Python comparator (candid-profile
  `guard/comparator.py`) and the Rust one (`crates/guard`) change together.
  The round-9 cases still match, and new cases with contacts match too.
- Unit tests of the look-alike rule.
- PocketIC: a `SetContact` cannot run before its timelock; a proposer cannot
  propose one; an upgrade keeps the contacts; a proposer's transfer to an
  unknown address is refused, one to a contact or to the address the request
  writes is not, and `Any` lets it through.
- candid-profile, with no paid API calls and a protocol note written before
  either run:
  - the attack benchmark's scripted follower on seed `attack-v0` with the
    trials' contacts in the guard's context, as a new run beside v1 (whose
    protocol is frozen);
  - the full guard benchmark (`data-r10q`: 12,000 requests, 56,937 checks)
    scored again with each request's named recipient as a contact. Round 9
    with the label reading today: 0 of 32,044 changes missed, and 275 of
    5,440 correct transfers flagged (5.1%, all memo reads). The false flags
    must not rise.

## Results, 2026-11-04

candid-profile branch `claude/project-thread-dhw2se`,
`guard/bench/contacts/` (protocol committed before either run; not
published). The plumbing check reproduced v0's and v1's figures first.

| Measure | Before | With contacts |
| --- | --- | --- |
| Follower's tricked proposals flagged by round 9 | 180 of 276 | 221 of 276 |
| Of those, the request names a person | 22 of 96 | 63 of 96 |
| Flagged by round 9 or refused by the recipient rule | | 253 of 276 (86 of 96 named) |
| Guard benchmark, round 9 label reading, missed | 0 of 32,044 | 0 of 32,044 |
| Guard benchmark, false flags | 275 of 5,440 | 275 of 5,440 |
| Guard benchmark, named-recipient changes flagged | 154 of 3,652 | 2,650 of 3,652 |

Swapping contact names before the model reads raised the false flags to 288
of 5,440, so the wallet does not swap them. The 23 tricked proposals that
neither check catches change the amount or the asset.
