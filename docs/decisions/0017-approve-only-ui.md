# 0017: One page to approve, and settings to set up

Status: accepted, 2026-11-06. Behrad chose "Approve-only screen" over
fixing how pages load, over a native Rust app and over keeping every page
(thread "Cut wallet UI to an approve-only screen"). He then chose to keep
one settings page for what only an owner can do.

**Decision: the wallet's start page is the only page a member uses day to
day. It shows what the wallet holds, each account with its addresses to
copy, the open actions with Approve, Run and Reject, and the requests to
the agent with their terms to confirm. The send, receive, account,
approvals and inbox pages, the menu of pages and the rail beside them are
gone. Settings, with its members page, keeps what only an owner can do:
members and the policy, accounts and tracked tokens, contacts, the guard's
model, the recovery controller and upgrades. `/approvals` still opens the
start page, so the links the agent and the scripts print keep working.
The wallet's interface and what it checks do not change.**

The wallet is AI first. An agent proposes each transfer, the guard checks
it against the owner's words, and the owner approves it
([decision 0007](0007-agent-proposer.md),
[decision 0012](0012-guard-in-wallet.md),
[decision 0014](0014-confirmed-terms.md)). The owner's part is to write the
request, confirm its terms, and approve and run what the agent proposes,
and the start page now holds all of it. The other pages made the wallet
feel slow. Each link was a full page load, and every load re-read the
wallet with replicated calls, which take seconds each on mainnet. One page
means fewer loads, and fewer reads per load.

What an owner loses in the browser, and where it is instead:

| Was | Now |
| --- | --- |
| The send page proposed and ran a transfer | The agent proposes it for a request the owner wrote on the start page (`wallet_propose_transfer`), and the owner approves and runs it there. Without an agent, the wallet sends nothing. |
| The receive page showed each address in full with its QR code | Each account on the start page copies its address, and an Internet Computer account its ICP account identifier, with what a sender must know. No QR code. |
| Each account's page showed its holdings and open transfers | The start page shows each account's balances; the agent reads history with `wallet_activity`. |
| The inbox listed the audit log and what was new | The health chip in the bar says what needs you; the agent reads the log. |
| Adding and renaming accounts, and tracking tokens, were on the start page | They are in settings, under Accounts. |

Why settings stays: an agent can propose only transfers
(`canisters/wallet/src/lib.rs`, "a proposer proposes only transfers"), and
contacts only an owner may propose ([decision 0015](0015-contacts.md)).
Internet Identity gives each site its own principal, so an owner can act
only from a browser at the wallet's own address, never from a CLI. Without
settings, no owner could install a signed upgrade, change the policy,
change the contacts, load the guard or hand control to a recovery
controller.

What changes, and what checks it:

- `ui/`: `logic/page.ts` lists the start page, settings, members, the
  claim and the 404 page. The frame is one bar along the top: the brand, the
  way to settings and who is signed in. The open actions moved from the
  approvals page into `parts/approve.tsx`, unchanged. The code only the
  dropped pages used is gone. That is the inbox's memory of what you saw,
  the rail and their styles. `app.js` went from 592,882 to 559,339 bytes
  and `app.css` from 41,721 to 28,350.
- `canisters/wallet/src/http.rs` serves the start page at `/`,
  `/index.html` and `/approvals`, and settings, members and the claim. Any
  other path, `/send` among them, is the certified 404. The router's tests
  and the PocketIC page check pin both.
- The MCP server's tool descriptions and errors tell the agent that an
  owner approves on the wallet's start page.
- Not changed: the Candid interface (one comment aside), the policy, the
  guard, and every check the wallet makes on a call.
