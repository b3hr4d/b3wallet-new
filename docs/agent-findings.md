# Findings from running the agent on mainnet

A list of what to fix, from Claude using `mcp/` as a proposer on a real
wallet. New findings go at the end of the list. When one is fixed, mark it
with the PR that fixed it, and keep it in the list.

## The run of 2026-11-06

- Wallet: `w7ojd-oyaaa-aaaay-aad6a-cai` on mainnet, at version 0.1.0.
- Agent: the MCP server at `078fc4a`, driven over stdio. Its principal is
  `uty22-m2jsu-ecvii-ukeja-re5s5-f5gdg-7tv2e-vvp3n-amf46-dgd5z-kae`, its
  label "Claude", and its role Proposer.
- Token: TESTICP (ledger `xafvr-biaaa-aaaai-aql5q-cai`). Its tokens came
  from the public faucet `nqoci-rqaaa-aaaap-qp53q-cai`, method
  `transfer_icp(text)`, at 10 TESTICP per call.

These steps worked:

1. The agent asked to join. The owner approved and ran the join from a
   phone, and the join code matched (7790 1156).
2. The owner tracked TESTICP and added the account "Savings".
3. Requests 2 and 3 each went the whole way: the agent wrote the terms, the
   owner confirmed them, the agent proposed the transfer, and the owner
   approved and ran it. The balances afterwards were right: ICP wallet
   10 → 4.9999 → 14.9999 (faucet) → 9.9998, and Savings 0 → 5 → 10.

## Findings

### 1. Nobody but the page that ran a transfer sees its outcome

The Approvals page shows "Sent: block N of the ledger", but the agent can't
learn whether a transfer it proposed succeeded.

- The canister records the outcome. `finish()` sets the action's state to
  `Executed(Sent(receipt))` or `Failed(reason)`
  (`canisters/wallet/src/actions.rs`), and an ICRC transfer also stores a
  transfer with `tx_id` (the block index) and the action id
  (`canisters/wallet/src/sending.rs`).
- The audit event is only `Executed : ActionId` (`candid/wallet.did`), and
  it is logged the same way on success and on failure (`fail()` in
  `sending.rs`).
- The page has the receipt only because it reads the reply to its own
  `execute` (`ui/src/logic/describe.ts`).
- The MCP server can't read it at all. `wallet_activity` reads only
  `audit_log`, and says "ran action N, which may have failed: the log does
  not say" (`mcp/src/tools.ts`). `wallet_pending` lists only open actions, so
  a finished action disappears from it. No tool reads `history`, and the
  canister has no query that reads one action by its id. In this run the
  agent could confirm actions 2 and 3 only from the balances. The same holds
  for the join (action 1).
- Fix: carry the outcome in the audit event, for example
  `Executed : record { id; outcome : variant { Sent : TxReceipt; Done; Failed : text } }`,
  or add a member query `action(id)`. Then `wallet_activity` can say
  "ran action 2: sent, block N" or "ran action 2: failed: …". A smaller
  first step is a `wallet_history` tool over `history`, which already has
  the `tx_id` and the action.

### 2. The guard checks little or nothing for a tracked ICRC-1 token

- "Send 5 TEST-ICP to our Savings" (request 2): the guard abstained ("The
  guard was not sure how to read your words"), and amount, asset, chain,
  from, to, memo and fee were all left unchecked.
- "Send 5 TESTICP to Savings" (request 3): it checked `to` only. Amount,
  asset, chain, from, memo and fee were left unchecked.
- My guess, not yet verified: the guard doesn't know tracked token symbols
  such as TESTICP, so it can't tie "5" to an asset. The hyphen and "our
  Savings" made it worse. Next, try the same sentence with "ICP", and read
  `guard_check` for both actions to see which slots it found.
- What must not change: abstaining adds no flags and blocks nothing.
  "Within the terms" was the check that held in both cases.

### 3. A request without an end makes the agent stop and ask

Neither request said how long it stays good, so the agent had to ask before
writing terms, as `wallet_propose_terms` tells it to. The owner was testing
from a phone and pushed back: "Why do you need such prompt here? I wanted to
test it now!" The agent then wrote "2 hours", and the owner confirmed it.

- Fix: when the owner writes a request on the Approvals page, ask for its end
  there, with a short choice (1 hour, today, 1 week), so the request's own
  words always carry it and the agent never has to guess or ask.

### 4. A request that isn't a transfer has no answer

Request 1 was "Check my icp balance please". The agent read the balance, but
a proposer has no way to answer a request except with terms or a transfer.
The request stayed open with no answers until the owner closed it.

- Fix: let a proposer answer a request with text, such as
  `answer_request(id, text)`, which also closes it, or let the owner mark a
  request as a question.

### 5. Nothing warns that a transfer is more than the account holds

Request 3 asked for 5 TESTICP when ICP wallet held 4.9999, and 5 plus the
0.0001 fee is 5.0001. The agent noticed this and got more from the faucet
first. Neither `wallet_propose_transfer` nor the terms said anything.

- Not yet checked: whether `propose` refuses it, or the ledger only refuses
  it when the transfer runs.
- Fix: `propose_transfer` (or the Approvals page) should say when the amount
  plus the fee is more than the balance it reads.

### 6. Small things in the MCP server's output

- `wallet_activity` shows other members as bare principals (`k6asx-…`).
  Only the agent itself becomes "you". Use member labels, as the page does.
- `wallet_accounts` and `wallet_balance` call a tracked token "ICRC-1 token
  of ledger xafvr-…", although the ledger's `icrc1_name` is "Test ICP". Use
  the name, or at least the symbol.

### 7. Trying the agent on mainnet needs test tokens

The TESTICP ledger and its faucet worked well for a real run without real
money. `scripts/try-mainnet.sh` and `mcp/README.md` could say how: track
`xafvr-biaaa-aaaai-aql5q-cai`, then call
`icp canister call -n ic --identity anonymous nqoci-rqaaa-aaaap-qp53q-cai transfer_icp '("<wallet principal>")'`
(10 TESTICP per call), and write requests in TESTICP.
