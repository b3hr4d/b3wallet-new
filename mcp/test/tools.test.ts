import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import { test } from "node:test"
import { createTestClient, type TestHandlers } from "@ic-reactor/core/testing"
import { Ed25519KeyIdentity } from "@icp-sdk/core/identity"
import { actor as ledgerActor, type Actor as Ledger } from "../src/generated/icrc1.ts"
import {
  actor as walletActor,
  type Account,
  type Action,
  type Actor as Wallet,
  type Chain,
  type Pending,
} from "../src/generated/wallet.ts"
import { readConfig } from "../src/config.ts"
import { joinCode } from "../src/join-code.ts"
import { InvalidArguments } from "../src/protocol.ts"
import { walletTools } from "../src/tools.ts"

const WALLET = "uxrrr-q7777-77774-qaaaq-cai"
const ICP_LEDGER = "ryjl3-tyaaa-aaaaa-aaaba-cai"
const TOKEN_LEDGER = "mxzaz-hqaaa-aaaar-qaada-cai"
const FRIEND = "2vxsx-fae"
const OWNER = "aaaaa-aa"
const SECOND = 1_000_000_000n

const icp: Account = {
  id: 1,
  kind: { tag: "InternetComputer" },
  label: "main",
  address: WALLET,
  icp_account_id: "1c7a48ba6a562aa9eaa2481a9049cdf0433b9738c992d698c31d8abf89cadc79",
  index: 0,
  holder: { tag: "Wallet" },
}

const btc: Account = {
  id: 2,
  kind: { tag: "Bitcoin", value: { tag: "Regtest" } },
  label: "Main",
  address: "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080",
  icp_account_id: null,
  index: 0,
  holder: { tag: "Wallet" },
}

const evm: Account = {
  id: 3,
  kind: { tag: "Evm" },
  label: "Main",
  address: "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
  icp_account_id: null,
  index: 0,
  holder: { tag: "Wallet" },
}

const token: Chain = { tag: "Icrc", value: TOKEN_LEDGER as Pending["proposed_by"] }
const base: Chain = { tag: "Evm", value: 8_453n }

/**
 * A wallet that keeps what is proposed and refuses what the real one refuses
 * a proposer, and records every method called on it.
 */
function setUp(options: { member?: boolean; accounts?: Account[] } = {}) {
  const agent = Ed25519KeyIdentity.generate(new Uint8Array(32).fill(7))
  const { client, mock, requests } = createTestClient({ identity: agent })
  const calls: string[] = []
  const proposals: Action[] = []
  const joins: unknown[][] = []
  const pending = (
    id: bigint,
    action: Action,
    by: string,
    request: bigint | null = null,
  ): Pending => ({
    id,
    action,
    proposed_by: by as Pending["proposed_by"],
    proposed_at_ns: 1_800_000_000n * SECOND,
    approvals: [],
    quorum: 1,
    executable_after_ns: 0n,
    expires_at_ns: 1_800_604_800n * SECOND,
    state: { tag: "Waiting" },
    request,
  })
  const record = <A extends unknown[], R>(name: string, run: (...args: A) => R) => {
    return (...args: A): R => {
      calls.push(name)
      return run(...args)
    }
  }
  const forbidden = (name: string) =>
    record(name, () => ({
      tag: "Err" as const,
      value: { tag: "Forbidden" as const, value: `not ${name}` },
    }))
  const handlers: TestHandlers<Wallet> = {
    status: record("status", () => ({
      version: "0.1.0",
      claimed: true,
      claim_expires_at_ns: null,
      cycles: 5_000_000_000_000n,
      upgrade: { tag: "Idle" },
      bitcoin_network: { tag: "Regtest" },
      release_channel: ICP_LEDGER as Pending["proposed_by"],
    })),
    whoami: record("whoami", (ctx) =>
      options.member
        ? {
            id: ctx.caller,
            role: { tag: "Proposer" },
            label: "Claude",
            added_at_ns: 1_700_000_000n * SECOND,
            added_by: null,
          }
        : null,
    ),
    request_join: record("request_join", (label, role, ctx) => {
      joins.push([label, role, ctx.caller])
      return {
        tag: "Ok",
        value: pending(
          9n,
          { tag: "AddMember", value: { id: ctx.caller, role: role ?? { tag: "Viewer" }, label } },
          ctx.caller,
        ),
      }
    }),
    accounts: record("accounts", () => ({
      tag: "Ok",
      value: options.accounts ?? [icp, btc, evm],
    })),
    assets: record("assets", () => ({
      tag: "Ok",
      value: [{ tag: "Icp" }, { tag: "Evm", value: 1n }, token, base],
    })),
    balance: record("balance", (id, asset) => ({
      tag: "Ok",
      value: {
        account: id,
        asset: asset ?? { tag: "Icp" },
        amount: 1_050_000_000n,
        as_of_ns: 1_800_000_000n * SECOND,
      },
    })),
    audit_log: record("audit_log", (request, ctx) => ({
      tag: "Ok",
      value: [
        {
          seq: 4n,
          at_ns: 1_800_000_060n * SECOND,
          by: OWNER as Pending["proposed_by"],
          event: { tag: "Executed", value: 2n },
        },
        {
          seq: 3n,
          at_ns: 1_800_000_000n * SECOND,
          by: ctx.caller,
          event: { tag: "Proposed", value: 2n },
        },
        { seq: 2n, at_ns: 0n, by: null, event: { tag: "CyclesLow", value: BigInt(request.limit) } },
      ],
    })),
    pending: record("pending", () => ({ tag: "Ok", value: [] })),
    propose: record("propose", (action, ctx) => {
      if (action.tag !== "Transfer") {
        return {
          tag: "Err",
          value: { tag: "Forbidden", value: "a proposer proposes only transfers" },
        }
      }
      proposals.push(action)
      return { tag: "Ok", value: pending(BigInt(proposals.length), action, ctx.caller) }
    }),
    requests: record("requests", () => ({
      tag: "Ok",
      value: [
        {
          id: 3n,
          text: `Send 1.5 ICP to ${FRIEND}`,
          asked_by: OWNER as Pending["proposed_by"],
          asked_at_ns: 1_800_000_000n * SECOND,
          open: true,
          answers: [],
        },
      ],
    })),
    propose_for: record("propose_for", (request, action, ctx) => {
      proposals.push(action)
      return { tag: "Ok", value: pending(BigInt(proposals.length), action, ctx.caller, request) }
    }),
    approve: forbidden("approve"),
    execute: forbidden("execute"),
    reject: forbidden("reject"),
  }
  mock<Wallet>(walletActor, WALLET, handlers)
  mock<Ledger>(ledgerActor, ICP_LEDGER, {
    icrc1_metadata: () => [
      ["icrc1:symbol", { tag: "Text", value: "ICP" }],
      ["icrc1:decimals", { tag: "Nat", value: 8n }],
      ["icrc1:fee", { tag: "Nat", value: 10_000n }],
    ],
  })
  mock<Ledger>(ledgerActor, TOKEN_LEDGER, {
    icrc1_metadata: () => [
      ["icrc1:symbol", { tag: "Text", value: "ckBTC" }],
      ["icrc1:decimals", { tag: "Nat", value: 8n }],
    ],
  })
  const config = readConfig({ B3WALLET_ID: WALLET, B3WALLET_NETWORK: "local" })
  const tools = new Map(walletTools(client, config).map((tool) => [tool.name, tool]))
  const run = (name: string, args: Record<string, unknown> = {}) => {
    const tool = tools.get(name)
    assert.ok(tool, name)
    return tool.run(args)
  }
  /** The wallet's methods the tools called, as the fake replica saw them. */
  const called = () =>
    new Set(
      requests
        .filter((request) => request.canisterId === WALLET && request.methodName)
        .map((request) => request.methodName),
    )
  return { client, agent, tools, run, calls, called, proposals, joins }
}

test("the tools read and propose, and none approves, runs or rejects", async () => {
  const { client, tools, run, calls, called } = setUp({ member: true })
  assert.deepEqual(
    [...tools.keys()],
    [
      "wallet_whoami",
      "wallet_join",
      "wallet_accounts",
      "wallet_balance",
      "wallet_activity",
      "wallet_pending",
      "wallet_requests",
      "wallet_propose_transfer",
    ],
  )
  for (const name of tools.keys()) {
    const args: Record<string, Record<string, unknown>> = {
      wallet_balance: { account: 1 },
      wallet_propose_transfer: { from: 1, to: FRIEND, amount: "1" },
    }
    await run(name, args[name] ?? {})
  }
  assert.deepEqual(
    calls.filter((name) => ["approve", "execute", "reject"].includes(name)),
    [],
  )
  // The interface file names methods the wallet does not export yet, which
  // the fake would answer; the real one has only these.
  const source = await readFile(
    new URL("../../canisters/wallet/src/lib.rs", import.meta.url),
    "utf8",
  )
  const exported = new Set(
    [...source.matchAll(/#\[ic_cdk::(?:query|update)[^\]]*\]\s*(?:async\s+)?fn\s+(\w+)/g)].map(
      (match) => match[1],
    ),
  )
  assert.ok(exported.has("propose"), "the wallet's methods are found")
  assert.deepEqual(
    [...called()].filter((name) => !exported.has(name!)),
    [],
  )
  client.dispose()
})

test("a transfer is proposed in base units of the account's token, and waits", async () => {
  const { client, run, proposals } = setUp({ member: true })
  const result = JSON.parse(
    await run("wallet_propose_transfer", {
      from: 1,
      to: ` ${FRIEND} `,
      amount: "1.5",
      memo: "rent",
    }),
  )
  assert.deepEqual(proposals, [
    {
      tag: "Transfer",
      value: {
        from: 1,
        asset: { tag: "Icp" },
        to: FRIEND,
        amount: 150_000_000n,
        fee: null,
        memo: new TextEncoder().encode("rent"),
      },
    },
  ])
  assert.equal(result.action, "1")
  assert.equal(result.what, `Send 1.5 ICP from account 1 (“main”) to ${FRIEND}, memo 72656e74`)
  assert.equal(result.state, "Waiting for approvals: 0 of 1")
  assert.equal(result.proposed_by, "you")
  assert.equal(result.base_units, "150000000")
  assert.match(result.next, /Nothing is sent yet/)
  assert.match(result.next, new RegExp(`http://${WALLET}.localhost:8000/approvals`))

  // A tracked token, named by its ledger, goes in its own units.
  const sent = JSON.parse(
    await run("wallet_propose_transfer", {
      from: 1,
      asset: TOKEN_LEDGER,
      to: FRIEND,
      amount: "0.5",
    }),
  )
  assert.deepEqual(proposals[1], {
    tag: "Transfer",
    value: { from: 1, asset: token, to: FRIEND, amount: 50_000_000n, fee: null, memo: null },
  })
  assert.equal(sent.what, `Send 0.5 ckBTC from account 1 (“main”) to ${FRIEND}`)

  // An EVM transfer names its chain, by name or chain id, in 18 decimals.
  for (const chain of ["base", "8453"]) {
    const onBase = JSON.parse(
      await run("wallet_propose_transfer", {
        from: 3,
        asset: chain,
        to: evm.address,
        amount: "0.25",
      }),
    )
    assert.equal(onBase.what, `Send 0.25 ETH on Base from account 3 (“Main”) to ${evm.address}`)
  }
  assert.deepEqual(proposals[2], {
    tag: "Transfer",
    value: {
      from: 3,
      asset: base,
      to: evm.address,
      amount: 250_000_000_000_000_000n,
      fee: null,
      memo: null,
    },
  })
  client.dispose()
})

test("a transfer takes the wallet's own account names, by the names rule", async () => {
  const savings: Account = {
    ...icp,
    id: 4,
    label: "BTC  Savings",
    address: `${WALLET}-6cc627i.4`,
    index: 4,
  }
  const spoof: Account = { ...icp, id: 5, label: FRIEND, address: `${WALLET}-6cc627i.5`, index: 5 }
  const { client, run, proposals } = setUp({
    member: true,
    accounts: [icp, btc, evm, savings, spoof],
  })
  const propose = async (args: Record<string, unknown>) =>
    JSON.parse(await run("wallet_propose_transfer", { amount: "1.5", ...args }))

  // "Main" names the ICP, Bitcoin and EVM accounts: without an asset it is two of them, so the agent must ask.
  await assert.rejects(propose({ from: "Main", to: FRIEND }), {
    message:
      /2 accounts are named “Main” \(1, Internet Computer.*; 2, .*\): ask the person which one/,
  })
  // The asset says which: ICP leaves the Internet Computer account. A name sends to its account's address.
  const sent = await propose({ from: " main ", asset: "ICP", to: "btc savings" })
  assert.deepEqual(proposals.at(-1), {
    tag: "Transfer",
    value: {
      from: 1,
      asset: { tag: "Icp" },
      to: savings.address,
      amount: 150_000_000n,
      fee: null,
      memo: null,
    },
  })
  assert.deepEqual(sent.names, [
    "from “main” is account 1",
    `to “btc savings” is account 4, ${savings.address}`,
  ])
  assert.equal(sent.what, `Send 1.5 ICP from account 1 (“main”) to ${savings.address}`)
  assert.equal(
    (await propose({ from: "Main", asset: "Base", to: evm.address })).names[0],
    "from “Main” is account 3",
  )
  assert.equal((await propose({ from: "1", to: FRIEND })).names, undefined) // an id in a string is an id

  // An address is an address, also when an account's label spells it; such a label never names an account.
  await propose({ from: 1, to: FRIEND })
  assert.equal((proposals.at(-1) as { value: { to: string } }).value.to, FRIEND)
  await assert.rejects(propose({ from: FRIEND, to: FRIEND }), {
    message: /no account is named “2vxsx-fae”/,
  })
  const before = proposals.length
  await assert.rejects(propose({ from: 1, to: "Vault" }), {
    message: /no account is named “Vault”/,
  })
  assert.equal(proposals.length, before)
  client.dispose()
})

test("an amount the token cannot hold is refused before anything is proposed", async () => {
  const { client, run, proposals } = setUp({ member: true })
  for (const [amount, reason] of [
    ["0.123456789", /amount: /],
    ["-1", /amount: /],
    ["1e3", /amount: /],
    ["0", /amount is zero/],
  ] as const) {
    await assert.rejects(
      run("wallet_propose_transfer", { from: 1, to: FRIEND, amount }),
      (error: unknown) => error instanceof InvalidArguments && reason.test(error.message),
    )
  }
  await assert.rejects(
    run("wallet_propose_transfer", { from: 4, to: FRIEND, amount: "1" }),
    /the wallet has no account 4/,
  )
  await assert.rejects(
    run("wallet_propose_transfer", { from: 1, to: FRIEND, amount: "1", memo: "x".repeat(33) }),
    /memo is 33 bytes/,
  )
  await assert.rejects(
    run("wallet_propose_transfer", { from: 1, to: " ", amount: "1" }),
    /to is empty/,
  )
  await assert.rejects(
    run("wallet_propose_transfer", { from: 1, asset: "BTC", to: FRIEND, amount: "1" }),
    /asset: “BTC” is neither ICP nor a ledger's canister id/,
  )
  for (const asset of [undefined, "BNB", "56"]) {
    await assert.rejects(
      run("wallet_propose_transfer", { from: 3, asset, to: evm.address, amount: "1" }),
      /asset: an EVM account needs its chain, one of Ethereum, Sepolia/,
    )
  }
  await assert.rejects(
    run("wallet_propose_transfer", {
      from: 3,
      asset: "Ethereum",
      to: evm.address,
      amount: "0.0000000000000000001",
    }),
    /amount: /,
  )
  assert.deepEqual(proposals, [])
  client.dispose()
})

test("joining asks to be a proposer, and returns the code the Approvals page shows", async () => {
  const { client, agent, run, joins } = setUp()
  const me = agent.getPrincipal().toText()
  const before = JSON.parse(await run("wallet_whoami"))
  assert.equal(before.you, me)
  assert.equal(before.member, null)
  assert.match(before.next, /wallet_join/)

  const result = JSON.parse(await run("wallet_join", { label: "Claude on a Mac" }))
  assert.deepEqual(joins, [["Claude on a Mac", { tag: "Proposer" }, me]])
  assert.equal(result.asks_to_be, "Proposer")
  assert.equal(result.code, joinCode(me))
  assert.match(result.next, new RegExp(joinCode(me)))
  await run("wallet_join")
  assert.deepEqual(joins[1], ["AI agent", { tag: "Proposer" }, me])
  client.dispose()
})

test("reads come back in the token's units", async () => {
  const { client, run } = setUp({ member: true })
  assert.deepEqual(JSON.parse(await run("wallet_accounts")), [
    {
      id: 1,
      label: "main",
      kind: "Internet Computer: ICP and the tracked ICRC-1 tokens",
      address: WALLET,
      icp_account_id: icp.icp_account_id,
      assets: [
        { asset: "ICP", name: "ICP", token: { symbol: "ICP", decimals: 8 } },
        {
          asset: TOKEN_LEDGER,
          name: `ICRC-1 token of ledger ${TOKEN_LEDGER}`,
          token: { symbol: "ckBTC", decimals: 8 },
        },
      ],
    },
    {
      id: 2,
      label: "Main",
      kind: "Bitcoin regtest",
      address: btc.address,
      icp_account_id: null,
      assets: [
        {
          asset: null,
          name: "Bitcoin regtest",
          token: { symbol: "regtest BTC", decimals: 8 },
        },
      ],
    },
    {
      id: 3,
      label: "Main",
      kind: "EVM: the native token of each tracked EVM chain, at one address",
      address: evm.address,
      icp_account_id: null,
      assets: [
        { asset: "Ethereum", name: "Ethereum", token: { symbol: "ETH", decimals: 18 } },
        { asset: "Base", name: "Base", token: { symbol: "ETH on Base", decimals: 18 } },
      ],
    },
  ])
  const balance = JSON.parse(await run("wallet_balance", { account: 1 }))
  assert.equal(balance.amount, "10.5 ICP")
  assert.equal(balance.base_units, "1050000000")
  const tokens = JSON.parse(await run("wallet_balance", { account: 1, asset: TOKEN_LEDGER }))
  assert.equal(tokens.amount, "10.5 ckBTC")
  assert.equal(tokens.asset, `ICRC-1 token of ledger ${TOKEN_LEDGER}`)
  const bitcoin = JSON.parse(await run("wallet_balance", { account: 2 }))
  assert.equal(bitcoin.amount, "10.5 regtest BTC")
  const ether = JSON.parse(await run("wallet_balance", { account: 3, asset: "Base" }))
  assert.equal(ether.amount, "0.00000000105 ETH on Base")
  assert.equal(ether.asset, "Base")
  assert.deepEqual(JSON.parse(await run("wallet_activity", { limit: 3 })), [
    {
      seq: "4",
      at: "2027-01-15T08:01:00.000Z",
      by: OWNER,
      event: "ran action 2, which may have failed: the log does not say",
    },
    { seq: "3", at: "2027-01-15T08:00:00.000Z", by: "you", event: "proposed action 2" },
    { seq: "2", at: "1970-01-01T00:00:00.000Z", by: "the wallet", event: "cycles ran low: 3" },
  ])
  client.dispose()
})

test("a refusal names the wallet's error", async () => {
  // The real wallet refuses a member that asks to join; this one is told to.
  const { client, mock } = createTestClient()
  mock<Wallet>(walletActor, WALLET, {
    request_join: () => ({
      tag: "Err",
      value: { tag: "InvalidArgument", value: "already a member" },
    }),
  })
  const join = walletTools(client, readConfig({ B3WALLET_ID: WALLET })).find(
    (tool) => tool.name === "wallet_join",
  )
  assert.ok(join)
  await assert.rejects(join.run({}), {
    message: "The wallet refused request_join: InvalidArgument: already a member",
  })
  client.dispose()
})

test("a transfer answers the owner's request by its id, so the guard can check it", async () => {
  const { client, run, calls } = setUp({ member: true })
  const requests = JSON.parse(await run("wallet_requests"))
  assert.deepEqual(requests, [
    {
      request: "3",
      text: `Send 1.5 ICP to ${FRIEND}`,
      asked_at: "2027-01-15T08:00:00.000Z",
      answers: [],
    },
  ])
  const result = JSON.parse(
    await run("wallet_propose_transfer", { request: 3, from: 1, to: FRIEND, amount: "1.5" }),
  )
  assert.equal(result.request, "3")
  assert.deepEqual(
    calls.filter((name) => name.startsWith("propose")),
    ["propose_for"],
  )
  client.dispose()
})
