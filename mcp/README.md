# b3wallet MCP server

Lets an AI agent, such as Claude, read one wallet and propose transfers from
it. The agent joins the wallet as a **proposer**: it sees the accounts,
balances, open actions and audit log, and proposes transfers, but its approval
never counts, so every transfer it proposes waits until an owner or an
approver approves and runs it on the wallet's Approvals page
([decision 0007](../docs/decisions/0007-agent-proposer.md)). The server offers
no tool that approves, runs or rejects anything, and the wallet would refuse
the agent if it tried.

The server runs on your computer, started by the AI client over stdio. On its
first run it makes the agent's own Ed25519 key in
`~/.config/b3wallet-mcp/agent.pem`, readable only by you, and the agent calls
the wallet with it. It needs Node 22.18 or later.

## Set it up

From the repository's root:

```sh
(cd mcp && npm ci)
```

`npm ci` also writes the wallet's bindings from `candid/wallet.did` into
`mcp/src/generated`. Run it again after the interface changes.

Add the server to Claude Code, with your wallet's id. For a wallet that
`scripts/try-local.sh` installed, add `-e B3WALLET_NETWORK=local`; the script
prints this command with both filled in.

```sh
claude mcp add b3wallet -e B3WALLET_ID=<wallet id> -- node "$PWD/mcp/src/main.ts"
```

For Claude Desktop, add it to `claude_desktop_config.json` (Settings,
Developer, Edit Config), with the full path of `node` (`which node`), since
the app does not read your shell's `PATH`:

```json
{
  "mcpServers": {
    "b3wallet": {
      "command": "/opt/homebrew/bin/node",
      "args": ["/path/to/b3wallet-new/mcp/src/main.ts"],
      "env": { "B3WALLET_ID": "<wallet id>", "B3WALLET_NETWORK": "local" }
    }
  }
}
```

| Variable           | Meaning                                                                   |
| ------------------ | ------------------------------------------------------------------------- |
| `B3WALLET_ID`      | The wallet canister's id. Required.                                       |
| `B3WALLET_NETWORK` | `ic` (the default) or `local`, icp-cli's network on port 8000.            |
| `B3WALLET_KEY`     | Where the agent's key is. Defaults to `~/.config/b3wallet-mcp/agent.pem`. |

## Let the agent in

Ask the AI to join the wallet. It calls `wallet_join` and tells you an
8-digit code. Open the wallet's Approvals page, check that the join request
asks for a proposer and shows that code, then approve it and run it. From
then on, ask it things such as "what does the wallet hold?" or "propose
sending 0.5 ICP to <principal>", and approve or reject each transfer it
proposes on the Approvals page, where its proposals show its label and role.

To see the agent's principal yourself, import its key into icp-cli:
`icp identity import agent --from-pem ~/.config/b3wallet-mcp/agent.pem`, then
`icp identity principal --identity agent`. To remove the agent, propose a
`RemoveMember` of that principal as an owner.

## Tools

| Tool                      | What it does                                                                     |
| ------------------------- | -------------------------------------------------------------------------------- |
| `wallet_whoami`           | The agent's principal, the wallet's version and pages, and the agent's role      |
| `wallet_join`             | Asks to join as a proposer and returns the code the Approvals page shows         |
| `wallet_accounts`         | The accounts, with each one's chain, address and token                           |
| `wallet_balance`          | One account's balance, in its token's units                                      |
| `wallet_activity`         | The audit log, newest first: who proposed, approved, ran or rejected what        |
| `wallet_pending`          | The open actions, who proposed them, and where each stands                       |
| `wallet_requests`         | The owners' open requests: what they ask the agent to do, in their own words     |
| `wallet_propose_transfer` | Proposes a transfer of a decimal amount, which waits for an owner or an approver |

A transfer names the owner's request it answers, so the wallet's guard checks
it against the owner's own words on the Approvals page
([decision 0012](../docs/decisions/0012-guard-in-wallet.md)); one that names no
request is shown as not checked.

Its `from` and `to` also take the label of one of the wallet's own accounts,
by candid-profile's names rule (decision D-045 there): the label matches after
Unicode NFKC, lowercase and collapsed spaces, and it must name exactly one
account, among those that can send the asset for `from`. "Main" with ICP
names the Internet Computer account; "Main" alone names two, so the server
refuses and the agent asks the person. An address is always an address, and a
label that spells one never names an account. The result lists what each name
stood for.

Every read is a replicated call, which the subnet certifies, as on the
wallet's pages.

## What a stolen key can do

Read the wallet, and propose up to 8 transfers at a time, which you see on the
Approvals page and can reject. It cannot move anything. Remove the agent and
delete the key file to stop it; the next run makes a new key, which must ask
to join again.

## Develop

```sh
npm run format:check && npm run lint && npm run typecheck && npm test
```

The MCP protocol (JSON-RPC over stdio, tools only) is in `src/protocol.ts`
rather than taken from the SDK, so the process that holds the agent's key runs
only this code and the IC libraries the wallet's pages already use. The tests
run the tools against a fake replica (`@ic-reactor/core/testing`) whose
wallet records every method called on it.
