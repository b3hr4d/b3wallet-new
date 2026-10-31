# 0014: An owner confirms the terms of a request before the agent's transfer

Status: proposed, 2026-10-31.

**Decision: before an agent answers an owner's request with a transfer, it
writes the terms it reads in the request: the token, who is paid, the most,
and the latest time the transfer may be sent. The wallet's page reads them
with Meant's `@meantjs/consent/terms`, pinned to the exact 0.5.0, and shows
them to the owners in words; an owner confirms them there, or says no. The
wallet refuses a transfer for a request whose terms wait for an owner or
were declined. The Approvals page checks the transfer against the confirmed
terms, and offers no Approve and no Run for one outside them. Every
transfer still waits for an owner or an approver
([decision 0007](0007-agent-proposer.md)).**

The guard of [decision 0012](0012-guard-in-wallet.md) reads the owner's
words with a small model and flags what differs. It only adds flags, it can
be unsure, and its words are all it has: "send 1.5 ICP" says nothing of how
long the request stays good. The Candid Profile spec's confirmed terms
(section 5.3, D-056 in Meant) close that gap without a model. The user
confirms, on the wallet's own screen, exactly what one task allows, and
each request is checked against that exactly, with nothing rounded. In
candid-profile's everyday-wording test (fact, its PR #71), the agent wrote
the terms from the person's everyday words and no one confirmed them. Two
small models signed 33 (Gemma 4 E2B) and 26 (E4B) of 650 bad requests:
where the words left a term open, they wrote "unlimited" or "never"
themselves, and the check then passed what the person had not allowed. The
person confirming the terms, which this record adds, is the step that test
named next.

| Option | What it is | For | Against |
| --- | --- | --- | --- |
| **A. Terms on the request, checked on the page** | `propose_terms` for the proposer, `confirm_terms` and `decline_terms` for owners, `request` for members. The wallet keeps the order and the record; the page reads the terms and checks the transfer with the library | Exact, and in words: the owner reads the whole account, the amount as written and the end in their own zone. A transfer outside the terms gets no Approve on the wallet's own page. A draft that cannot be read is refused, with why, before it is written. The check is the library every other consumer of the spec uses | The check runs on the page, not in the canister: an owner using another client can still approve a transfer outside the terms. `app.js` grows from 543,895 to 579,114 bytes, so its budget goes from 560,000 to 600,000. Two more packages ship in the signed release |
| B. A, and the canister requires terms | `propose_for` also refuses a request with no terms | No transfer answers a request without confirmed terms | Agents that do not write terms, and requests written before this change, stop working. It is a one-line change once every agent writes terms |
| C. The check in the canister | A Rust port of `checkGrant` runs in `approve` and `execute` | No client can approve a transfer outside the terms | A second implementation of the terms to keep equal to the library, as `meant-format` is for amounts. Reading the draft (dates, names, zones) stays on the page either way |

What changes with A:

- **The wallet.** `Request` gains `terms : opt RequestTerms`. It is an
  `opt`, so a request stored before this change reads with none
  (`storage::tests::a_request_stored_before_terms_has_none`).
  - `propose_terms(id, draft)`, for proposers, writes a draft: four fields of
    1 to 160 bytes with no control characters, at most 3 drafts per request,
    and none once a transfer answers it. A new draft drops any yes or no to
    the last one.
  - `confirm_terms(id, { version, terms, id, text })`, for owners, takes the
    terms as JSON (RFC 8785) of at most 2,048 bytes, the words the owner read
    (at most 600 bytes) and the terms' id. It refuses an id that is not the
    SHA-256 of the JSON in lowercase hex, and a version that is not the
    latest. `decline_terms(id, version)` records a no.
  - `request(id)`, a query for members, reads one request, open or closed.
  - `propose_for` refuses a request whose terms wait for an owner, or were
    declined.
  - The audit log gains `TermsProposed`, `TermsConfirmed` and
    `TermsDeclined`.

  The wallet does not read the terms. It keeps what the page confirmed, who
  confirmed it and when, and checks only that the record holds together.
- **The page.** The Requests card shows the agent's draft in words, read as
  of when it was written and in the browser's zone then, with "Confirm these
  terms" and "Not what I meant". A draft that cannot be read, or has already
  ended, offers only the no. The Approvals page reads the request a transfer
  answers and checks the transfer against its confirmed terms each time it
  renders, at least every 10 seconds. Outside them, Approve and Run are not
  offered and the reasons are listed; Reject stays. A transfer for a request
  without terms shows a notice and keeps its buttons, as before (that is B's
  difference).
- **Names.** Terms name a token by the symbol its ledger reports, and who is
  paid by an ICRC-1 account in its text form or the label of one of the
  wallet's own Internet Computer accounts. A name that would not name one
  thing is left out: ICP keeps its name, and another ledger that calls
  itself ICP is named by nothing, nor are two ledgers or two accounts that
  share a name once case, spaces and punctuation are set aside. Terms do not
  describe Bitcoin or an EVM chain's token, so such a transfer is outside
  any terms.
- **The agent's tools.** `wallet_propose_terms` reads the draft as the page
  will (in UTC), refuses one that cannot be read before it is written, and
  returns the words the owner will see. `wallet_requests` shows each
  request's terms and what an owner said. `wallet_propose_transfer` refuses
  a transfer outside the confirmed terms, with the reasons.
- **Packages.** `@meantjs/format` moves from 0.1.0 to 0.5.0 in `ui/` and
  `mcp/` ([decision 0013](0013-meant-format.md) pinned 0.1.0; the package did
  not change in behaviour the wallet uses, and the tests of amounts, memos
  and names pass as they were).
  `@meantjs/consent` 0.5.0 joins both, and `ui/` pins `@meantjs/profile`
  0.5.0 for the RFC 8785 JSON the terms are stored as. `ui/build.mjs` lists
  both new packages. `npm audit --omit=dev` finds nothing in either.

Not covered yet:

- **The check is the page's.** Another client of an owner can approve and
  run a transfer outside the terms (C).
- **The zone.** A confirmation names the zone of the owner's browser at the
  time of the draft. A date allows its whole day there.
- **The clock.** The page checks the end against the browser's clock when it
  shows the buttons; the wallet runs a transfer when an owner runs it.
