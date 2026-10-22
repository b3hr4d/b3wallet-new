// The tools an AI agent gets: read the wallet, ask to join it as a proposer,
// and propose transfers. None approves, runs or rejects anything, and the
// wallet would refuse a proposer that tried: every transfer the agent
// proposes waits for an owner or an approver on the wallet's Approvals page.
import { formatUnits, isReactorError, parseUnits, type Client } from "@ic-reactor/core"
import { actor as ledgerActor, type Actor as Ledger } from "./generated/icrc1.ts"
import {
  actor as walletActor,
  type Account,
  type Action,
  type Actor as Wallet,
  type AuditEvent,
  type Chain,
  type Pending,
} from "./generated/wallet.ts"
import type { Config } from "./config.ts"
import { joinCode } from "./join-code.ts"
import { InvalidArguments, type Tool } from "./protocol.ts"

/** The ICP ledger, at its mainnet id on every network. */
const ICP_LEDGER = "ryjl3-tyaaa-aaaaa-aaaba-cai"
/** The longest memo a transfer carries, in bytes. */
const MAX_MEMO_BYTES = 32

type Token = { symbol: string; decimals: number }

/** JSON for the model: bigints as decimal text, two spaces of indent. */
function json(value: unknown): string {
  return JSON.stringify(
    value,
    (_key, item: unknown) => (typeof item === "bigint" ? item.toString() : item),
    2,
  )
}

/** A time the wallet gives in nanoseconds since the epoch. */
function dateOf(ns: bigint): string {
  return new Date(Number(ns / 1_000_000n)).toISOString()
}

export function chainText(chain: Chain): string {
  switch (chain.tag) {
    case "Icp":
      return "ICP"
    case "Icrc":
      return `ICRC-1 token of ledger ${chain.value}`
    case "Bitcoin":
      return chain.value.tag === "Mainnet" ? "Bitcoin" : `Bitcoin ${chain.value.tag.toLowerCase()}`
    case "Evm":
      return `EVM chain ${chain.value}`
  }
}

/** Why a call failed, for the model: the wallet's own error when it refused. */
export function failureText(error: unknown): string {
  if (isReactorError(error) && error.kind === "canister_err") {
    const err = error.err as { tag: string; value?: unknown }
    const detail =
      err.value === undefined
        ? ""
        : `: ${typeof err.value === "string" ? err.value : json(err.value)}`
    return `The wallet refused ${error.method}: ${err.tag}${detail}`
  }
  return error instanceof Error ? error.message : String(error)
}

/** An audit log event, in a few words. */
export function eventText(event: AuditEvent): string {
  switch (event.tag) {
    case "Claimed":
      return "claimed the wallet"
    case "JoinRequested":
      return "asked to join"
    case "AccountAdded":
      return `added account ${event.value}`
    case "Proposed":
    case "Approved":
    case "Rejected":
    case "Executed":
      return `${event.tag.toLowerCase()} action ${event.value}`
    case "ExtensionCall":
      return `extension ${event.value.extension} asked: ${event.value.request}`
    case "UpgradeStaged":
      return `staged the upgrade to ${event.value}`
    case "UpgradeInstalled":
      return `installed version ${event.value}`
    case "UpgradeFailed":
      return `the upgrade failed: ${event.value}`
    case "CyclesLow":
      return `cycles ran low: ${event.value}`
  }
}

/** A string argument with its whitespace trimmed, or undefined when absent. */
function text(args: Record<string, unknown>, name: string): string | undefined {
  const value = args[name]
  return typeof value === "string" ? value.trim() : undefined
}

function integer(args: Record<string, unknown>, name: string): number | undefined {
  const value = args[name]
  return typeof value === "number" ? value : undefined
}

/**
 * The tools, each of which reports a refusal of the wallet by its error, such
 * as `Forbidden: a proposer proposes only transfers`.
 */
export function walletTools(client: Client, config: Config): Tool[] {
  return tools(client, config).map((tool) => ({
    ...tool,
    run: (args) =>
      tool.run(args).catch((error: unknown) => {
        throw error instanceof InvalidArguments ? error : new Error(failureText(error))
      }),
  }))
}

function tools(client: Client, config: Config): Tool[] {
  const wallet = client.canister<Wallet>(walletActor, { id: config.wallet, certified: true })
  const approvals = `${config.pages}/approvals`
  const tokens = new Map<string, Promise<Token | undefined>>()

  /** What a chain's accounts hold, read from its ledger once. */
  const tokenOf = (chain: Chain): Promise<Token | undefined> => {
    switch (chain.tag) {
      case "Bitcoin":
        return Promise.resolve({
          symbol: chain.value.tag === "Mainnet" ? "BTC" : `${chain.value.tag.toLowerCase()} BTC`,
          decimals: 8,
        })
      case "Evm":
        return Promise.resolve(undefined)
      case "Icp":
      case "Icrc": {
        const id = chain.tag === "Icp" ? ICP_LEDGER : chain.value
        let token = tokens.get(id)
        if (token === undefined) {
          const ledger = client.canister<Ledger>(ledgerActor, { id, certified: true })
          token = ledger.icrc1_metadata().then((metadata) => {
            const entry = (key: string) => metadata.find(([name]) => name === key)?.[1]
            const symbol = entry("icrc1:symbol")
            const decimals = entry("icrc1:decimals")
            if (symbol?.tag !== "Text" || decimals?.tag !== "Nat" || decimals.value > 255n) {
              return undefined
            }
            return { symbol: symbol.value, decimals: Number(decimals.value) }
          })
          // A failed read is tried again next time.
          token.catch(() => tokens.delete(id))
          tokens.set(id, token)
        }
        return token
      }
    }
  }

  const amountText = (amount: bigint, token: Token | undefined) =>
    token ? `${formatUnits(amount, token.decimals)} ${token.symbol}` : `${amount} base units`

  const accountOf = async (id: number): Promise<Account> => {
    const account = (await wallet.accounts()).find((account) => account.id === id)
    if (account === undefined) throw new InvalidArguments(`the wallet has no account ${id}`)
    return account
  }

  const describe = async (action: Action, accounts: readonly Account[]): Promise<string> => {
    switch (action.tag) {
      case "Transfer": {
        const { from, to, amount, fee, memo } = action.value
        const account = accounts.find((account) => account.id === from)
        const token = account && (await tokenOf(account.chain).catch(() => undefined))
        const source = account ? `account ${from} (“${account.label}”)` : `account ${from}`
        const extras = [
          fee === null ? "" : `, fee ${amountText(fee, token)}`,
          memo === null ? "" : `, memo ${Buffer.from(memo).toString("hex")}`,
        ].join("")
        return `Send ${amountText(amount, token)} from ${source} to ${to}${extras}`
      }
      case "AddMember":
        return `Add ${action.value.id} as ${action.value.role.tag}, “${action.value.label}”`
      case "ChangeRole":
        return `Make ${action.value.id} ${action.value.role.tag}`
      case "RemoveMember":
        return `Remove the member ${action.value}`
      default:
        return `${action.tag}: ${json(action.value)}`
    }
  }

  const pendingView = async (pending: Pending, accounts: readonly Account[]) => ({
    action: pending.id,
    what: await describe(pending.action, accounts),
    state:
      pending.state.tag === "Waiting"
        ? `Waiting for approvals: ${pending.approvals.length} of ${pending.quorum}`
        : pending.state.tag === "Ready" && pending.executable_after_ns > 0n
          ? `Ready to run from ${dateOf(pending.executable_after_ns)}`
          : "value" in pending.state
            ? { [pending.state.tag]: pending.state.value }
            : pending.state.tag,
    proposed_by: pending.proposed_by === client.caller() ? "you" : pending.proposed_by,
    proposed_at: dateOf(pending.proposed_at_ns),
    open_until: dateOf(pending.expires_at_ns),
  })

  return [
    {
      name: "wallet_whoami",
      title: "Who you are to the wallet",
      description:
        "Your principal, the wallet's id and pages, and your role in it: a proposer, or not a member yet. Call it first.",
      inputSchema: { type: "object", properties: {}, additionalProperties: false },
      readOnly: true,
      run: async () => {
        const status = await wallet.status()
        const me = status.claimed ? await wallet.whoami() : null
        return json({
          you: client.caller(),
          wallet: config.wallet,
          network: config.network,
          pages: config.pages,
          version: status.version,
          member: me && { role: me.role.tag, label: me.label, since: dateOf(me.added_at_ns) },
          next: !status.claimed
            ? "The wallet has no owner yet, so nobody can join it."
            : me === null
              ? "You are not a member. Call wallet_join, and give the user the code it returns."
              : me.role.tag === "Proposer"
                ? `You read the wallet and propose transfers. Each waits for an owner or an approver at ${approvals}.`
                : `You are ${me.role.tag}, but this server only reads and proposes.`,
        })
      },
    },
    {
      name: "wallet_join",
      title: "Ask to join the wallet",
      description:
        "Asks to join the wallet as a proposer: one who reads it and proposes transfers, and whose approval never counts. An owner approves the request on the wallet's Approvals page, after checking it shows the code this returns. Asking again while the request is open returns the same request.",
      inputSchema: {
        type: "object",
        properties: {
          label: {
            type: "string",
            description: "A name for this agent that the owners see, such as “Claude on my Mac”.",
          },
        },
        additionalProperties: false,
      },
      readOnly: false,
      run: async (args) => {
        const label = text(args, "label") || "AI agent"
        const pending = await wallet.request_join(label, { tag: "Proposer" })
        const role = pending.action.tag === "AddMember" ? pending.action.value.role.tag : "?"
        const code = joinCode(client.caller())
        return json({
          request: pending.id,
          you: client.caller(),
          asks_to_be: role,
          label: pending.action.tag === "AddMember" ? pending.action.value.label : label,
          code,
          open_until: dateOf(pending.expires_at_ns),
          next: `Tell the user to open ${approvals}, check that the join request shows the code ${code}, then approve it and run it. Once it has run, call wallet_whoami.`,
        })
      },
    },
    {
      name: "wallet_accounts",
      title: "The wallet's accounts",
      description:
        "The wallet's accounts: each one's id, label, chain, address and token. Transfers name the account they send from by its id.",
      inputSchema: { type: "object", properties: {}, additionalProperties: false },
      readOnly: true,
      run: async () => {
        const accounts = await wallet.accounts()
        return json(
          await Promise.all(
            accounts.map(async (account) => ({
              id: account.id,
              label: account.label,
              chain: chainText(account.chain),
              address: account.address,
              token: (await tokenOf(account.chain).catch(() => undefined)) ?? null,
            })),
          ),
        )
      },
    },
    {
      name: "wallet_balance",
      title: "An account's balance",
      description: "What one of the wallet's accounts holds now, as its chain reports it.",
      inputSchema: {
        type: "object",
        properties: {
          account: {
            type: "integer",
            description: "The account's id.",
            minimum: 0,
            maximum: 2 ** 32 - 1,
          },
        },
        required: ["account"],
        additionalProperties: false,
      },
      readOnly: true,
      run: async (args) => {
        const account = await accountOf(integer(args, "account") ?? 0)
        const [balance, token] = await Promise.all([
          wallet.balance(account.id),
          tokenOf(account.chain),
        ])
        return json({
          account: account.id,
          label: account.label,
          amount: amountText(balance.amount, token),
          base_units: balance.amount,
          as_of: dateOf(balance.as_of_ns),
        })
      },
    },
    {
      name: "wallet_activity",
      title: "What happened in the wallet",
      description:
        "The wallet's audit log, newest first: who proposed, approved, ran or rejected each action, who asked to join, and accounts added. A transfer that ran is no longer open: its action shows here as executed.",
      inputSchema: {
        type: "object",
        properties: {
          limit: {
            type: "integer",
            description: "At most this many entries. Defaults to 20.",
            minimum: 1,
            maximum: 100,
          },
          before_seq: {
            type: "integer",
            description: "Only entries before this sequence number, for the next page.",
            minimum: 0,
          },
        },
        additionalProperties: false,
      },
      readOnly: true,
      run: async (args) => {
        const before = integer(args, "before_seq")
        const entries = await wallet.audit_log({
          before_seq: before === undefined ? null : BigInt(before),
          limit: integer(args, "limit") ?? 20,
        })
        return json(
          entries.map((entry) => ({
            seq: entry.seq,
            at: dateOf(entry.at_ns),
            by: entry.by === null ? "the wallet" : entry.by === client.caller() ? "you" : entry.by,
            event: eventText(entry.event),
          })),
        )
      },
    },
    {
      name: "wallet_pending",
      title: "Open actions",
      description:
        "Every open action of the wallet, yours and the owners': what it does, who proposed it, and whether it waits for approvals or is ready to run.",
      inputSchema: { type: "object", properties: {}, additionalProperties: false },
      readOnly: true,
      run: async () => {
        const [open, accounts] = await Promise.all([wallet.pending(), wallet.accounts()])
        return json(await Promise.all(open.map((pending) => pendingView(pending, accounts))))
      },
    },
    {
      name: "wallet_propose_transfer",
      title: "Propose a transfer",
      description:
        "Proposes sending an amount from one of the wallet's accounts. It sends nothing: the transfer waits until an owner or an approver approves and runs it on the wallet's Approvals page, and expires after seven days. You can have at most 8 actions open at once.",
      inputSchema: {
        type: "object",
        properties: {
          from: {
            type: "integer",
            description: "The id of the account to send from, from wallet_accounts.",
            minimum: 0,
            maximum: 2 ** 32 - 1,
          },
          to: {
            type: "string",
            description:
              "For ICP and ICRC-1 tokens, an ICRC-1 account in its text form (a principal is one); for Bitcoin, an address of the account's network.",
          },
          amount: {
            type: "string",
            description:
              "The amount in the token's own units as decimal text, such as “1.5” for 1.5 ICP. The ledger's fee comes on top.",
          },
          memo: {
            type: "string",
            description: `A note of at most ${MAX_MEMO_BYTES} bytes of UTF-8 that the ledger records. Not for Bitcoin.`,
          },
        },
        required: ["from", "to", "amount"],
        additionalProperties: false,
      },
      readOnly: false,
      run: async (args) => {
        const account = await accountOf(integer(args, "from") ?? 0)
        const token = await tokenOf(account.chain)
        if (token === undefined) {
          throw new InvalidArguments(
            `this server cannot send from a ${chainText(account.chain)} account`,
          )
        }
        const to = text(args, "to") ?? ""
        if (!to) throw new InvalidArguments("to is empty")
        let amount: bigint
        try {
          amount = parseUnits(text(args, "amount") ?? "", token.decimals)
        } catch (error) {
          throw new InvalidArguments(
            `amount: ${error instanceof Error ? error.message : String(error)}`,
          )
        }
        if (amount === 0n) throw new InvalidArguments("amount is zero")
        const note = text(args, "memo")
        const memo = note ? new TextEncoder().encode(note) : null
        if (memo && memo.length > MAX_MEMO_BYTES) {
          throw new InvalidArguments(
            `memo is ${memo.length} bytes, and at most ${MAX_MEMO_BYTES} fit`,
          )
        }
        const pending = await wallet.propose({
          tag: "Transfer",
          value: { from: account.id, to, amount, fee: null, memo },
        })
        return json({
          ...(await pendingView(pending, [account])),
          base_units: amount,
          next: `Nothing is sent yet. Tell the user it waits for an owner or an approver to approve and run action ${pending.id} at ${approvals}.`,
        })
      },
    },
  ]
}
