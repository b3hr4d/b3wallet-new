# 0011: The guard runs inside the wallet

Status: accepted, 2026-10-26. Behrad chose it: "inside the same canister per user so they could say this model is
running in my own resource and never getting wrong by manipulation".

**Decision: each wallet holds the guard itself. The guard is the small model that reads the person's own words, plus the
deterministic checks that compare a proposed transfer with them. The model is one file the owners upload, and the
wallet loads only the file whose SHA-256 its code pins. Owners and approvers ask the guard about a pending transfer
with `guard_check`, a query, so asking is free.**

The guard comes from candid-profile (`packages/profile-router/guard/`). It is stage 1, the round-9 BERT-mini tagger
with its comparator and request rule, and stage 2, the keyword list cut to the 1% false-flag bar. `crates/guard`
is its Rust port.

| Option | What it is | For | Against |
| --- | --- | --- | --- |
| **A. In the wallet** | The model, the comparator, the request rule and the keyword list run in the owners' own canister | They run on the owners' own resources, under the code the release signers signed. An agent cannot change the model or answer for it | The module grows by about 0.6 MB (5.4 to 6.0 MB built) and the heap by 44 MB. Each upgrade reads the file again (about 6B cycles) |
| B. A guard canister beside it | The same model in a canister of its own; the checks run off-chain next to the approver | Nothing in the wallet changes | Someone else's canister answers. The checks run wherever the approver runs them |
| C. Off-chain, next to the agent | The Python guard on the agent's computer | No canister work | The agent's own machine would vouch for the agent |

What it does and does not do:

1. **It only adds flags for a person to read.** Nothing in it approves, rejects or changes an action, and
   `NotChecked` is never OK. A transfer whose request it cannot read is "partial", never "matches".
2. **The model is pinned.** The file is `guard::FILE_BYTES` (44,359,636) bytes long with SHA-256
   `guard::FILE_SHA256` (e9ebd2b0…), written by candid-profile's `guard/canister/export.py` from the round-9 release.
   - `guard_upload` writes owners' chunks at offsets into the guard's own stable memory (memory id 15).
   - `guard_load` hashes the whole file and refuses any other.
   - `post_upgrade` loads the file again, because a new release may pin another one.
   - Changing the model takes a release.
3. **Only owners and approvers check, and only as a query.**
   - `guard_check` refuses replicated execution, so it never charges the wallet. On the local network, 10 checks took
     the same cycles as 10 `guard_status` queries.
   - A proposer cannot call it, so an agent cannot use it to try out wordings before it proposes.
4. **The words come from the owner, never from the agent** (added 2026-10-26: Behrad asked that no one paste the
   request by hand). An owner writes what they ask their agent to do as a request in the wallet (`ask`, at most 280
   bytes, 16 open at once; the Approvals page has the box), signed in as themselves. The agent reads the open requests
   (`requests`; the MCP server's `wallet_requests`) and answers one with `propose_for`. `guard_check` takes only the
   action and reads the words of the request it answers. A transfer that answers none is refused as "answers no
   request of yours", and the page says the guard could not check it.
   - A request has at most one answer that is open or ran, so an agent cannot propose it twice. After a rejection,
     expiry or failure, another transfer may answer it.
   - Owners close a request when no transfer should answer it any more (`close_request`).
   - Writing and closing are in the audit log (`Asked`, `RequestClosed`), and so is loading the guard file
     (`GuardLoaded`).
5. **What it reads from the wallet:**
   - the pending transfer as stored (with its asset);
   - the listed accounts, their labels and addresses;
   - the proposer's member label and the memo, both run through the keyword list.
6. **The wallet's own names** (candid-profile decision D-045, added 2026-10-27): before the model reads the request,
   each listed account's label in it is swapped for a label the model was trained on, and the spans are mapped back
   (`Guard::answer_named`). The model then reads "my BTC Savings" as a label, not as a second asset. The comparator
   still resolves the label from the owner's own words. A label that is an address, an asset or chain word, or a
   token symbol is never swapped.

Checked:

- **The port** (`crates/guard/tests/parity.rs`) gives the shipped Python's answers on all of round 9's test split:
  12,000 requests, 56,743 proposals and 5,150 stage-2 texts. The repository keeps a 1-in-60 sample of them. With the
  file present, it also matches PyTorch's inputs and answers on the release's 300 parity cases, with logits within
  3.3e-5.
- **PocketIC** (`tests/integration/tests/wallet.rs`):
  - without the file it answers nothing, refuses a file other than the pinned one, refuses proposers, and refuses to
    run as an update;
  - with `GUARD_BIN` set, it matches a correct transfer, flags another amount and an added memo, and is loaded again
    after an upgrade;
  - with `GUARD_BIN` set, "Send 1.5 ICP to my BTC Savings" matches a transfer to that account and flags one to any
    other address (without the swap, the guard abstains on it).
- **Names** (candid-profile `guard/names-dev.json`, dev data only): on requests whose labels hold a token or chain
  word, the swap takes the share of amount, asset and to checked from 13 to 15% to 80 to 86%, and misses from 24 to
  2. On ordinary requests it checks 3 to 4 points more, with no new miss. The swap equals the Python on 608 cases
  (`tests/data/names.jsonl`).
- **Locally, end to end** (candid-profile `guard/canister/try/local.sh --in-wallet`): 20 transfers proposed by an
  agent through `propose`.
  - 0 false flags on the 7 honest ones;
  - all 8 planted changes caught;
  - none of the 5 misread requests passed as a match;
  - the same answers as the Python and PyTorch guard on all 20;
  - 175M to 299M instructions per check.
  - with the names swap (2026-10-27), the same 20 answers again, and 10 more around ICP accounts named "BTC Savings"
    and "Rent ETH": the 5 honest ones match, all 4 planted changes are caught and a "don't send" request is
    flagged, where the model alone abstains on all 9 transfers (candid-profile `try/results-names.json`).

Open:

- **The rail.** The Approvals page shows the guard's verdict on each ticket; the rail beside other pages does not yet.
- **Tokens.** The wallet keeps no ICRC-1 ledger's symbol or decimals, so an ICRC-1 transfer's asset and amount are not
  checked. Storing them when a ledger is tracked would fix that.
- **Queries come from one replica.** A query's answer is not certified. A faulty replica could leave a flag out, as it
  could answer any query wrongly. A certified check would be an update, which the wallet pays for.
- **Shipping the file.** For now the owners upload it. It could come with the release instead.
