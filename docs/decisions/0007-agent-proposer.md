# 0007: An AI agent joins as a proposer

Status: accepted, 2026-10-22. Behrad chose option A.

**Decision: a fourth role, `Proposer`, reads the wallet and proposes
transfers, and its approval never counts. An AI agent reaches the wallet
through an MCP server that runs on its owner's computer with its own key and
joins as a proposer, so every transfer it proposes waits for an owner or an
approver on the Approvals page.**

| Option | What it is | For | Against |
| --- | --- | --- | --- |
| **A. Propose only** | A `Proposer` role that reads and proposes transfers, never approves, runs or rejects | A real AI can do things while a person confirms each one; nothing it holds moves funds | A new role in the interface and the pages |
| B. Read only | The agent joins as a viewer | No change to the wallet | The agent cannot ask for anything |
| C. Wait | Agents come later as extension canisters with their own budget, as the design plans for v1 | No new surface now | Nothing to try until then |

What a proposer may do, and why:

1. **Read** what a viewer reads: the accounts, balances, open actions,
   audit log and members.
2. **Propose transfers, and nothing else.** A proposer's `propose` of any
   other action is refused (`Forbidden`), so it cannot ask to add a member,
   change a role or the policy, or upgrade the wallet.
3. **Its approval never counts.** A proposal starts with no approvals,
   whatever the quorum, so the daily-limit shortcut that releases an owner's
   small transfer at once does not release a proposer's: each one waits for
   an owner or an approver to approve and run it. `approve` and `execute` are
   for owners and approvers, and `reject` for owners, as before.
4. **At most 8 open actions** of its own, so an agent cannot fill the 64 open
   actions a wallet allows and crowd the owners out.
5. **It joins by asking.** `request_join` takes an optional role, `Viewer` or
   `Proposer`, and nothing else; absent, it asks to be a viewer, as before.
   The request is an `AddMember` the owners approve after checking its
   8-digit code, and the Approvals page says which role it asks for.

The MCP server (`mcp/`) makes its Ed25519 key on first run, in a file only
its user can read, and offers the agent tools that read and propose; none
approves, runs or rejects. A stolen key can read the wallet and propose up to
8 transfers that the owners see on the Approvals page and can reject; it
cannot move anything. An owner removes the agent with a `RemoveMember`
action.
