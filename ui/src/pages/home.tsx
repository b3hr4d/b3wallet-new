import type { Principal } from "@candid-core/schema"
import { formatUnits } from "@ic-reactor/core"
import { useQueries, useQuery } from "@tanstack/react-query"
import type { ReactNode } from "react"
import type { Member } from "../generated/wallet.ts"
import { useWallet } from "../ic/canister.ts"
import { failureText } from "../logic/describe.ts"
import { hueClass } from "../logic/mark.ts"
import { dateOf } from "../logic/time.ts"
import { assetsOf, ICP_ONLY } from "../logic/tokens.ts"
import { Accounts, Holdings, type BalanceRead } from "../parts/accounts.tsx"
import { OpenActions } from "../parts/approve.tsx"
import { CopyButton } from "../parts/copy.tsx"
import { Requests } from "../parts/guard.tsx"
import { WalletMark } from "../parts/icons.tsx"
import { JoinRequest } from "../parts/join.tsx"
import { Reading, useMe } from "../parts/member.tsx"
import { HomeOutline } from "../parts/outline.tsx"
import { SignIn } from "../parts/sign-in.tsx"

/**
 * The wallet's start page, and for a member the only page used day to day
 * (decision 0017): what the wallet holds, what you ask your agent, the open
 * actions to approve and run, and the accounts with their addresses. An
 * agent proposes every transfer; setting the wallet up is in settings.
 * Anyone else reads what the wallet runs, and signs in or asks to join.
 */
export function Home({ wallet: id }: { wallet: Principal }) {
  const { auth, status, claimed, me } = useMe(id)

  if (status.isError) {
    return (
      <div className="overview">
        <Hero id={id} />
        <p className="card error">{failureText(status.error)}</p>
      </div>
    )
  }
  if (status.data === undefined) return <HomeOutline />
  if (claimed && auth.status === "signed-in" && me.isSuccess && me.data !== null) {
    return <Dashboard wallet={id} me={me.data} />
  }
  let body: ReactNode
  if (!claimed) {
    body = (
      <div className="card empty">
        <h2>No owner yet</h2>
        <p className="quiet">
          Whoever opens this wallet&apos;s claim link and signs in becomes its first owner.
        </p>
        {status.data.claim_expires_at_ns !== null && (
          <p className="chip">
            The claim link works until {dateOf(status.data.claim_expires_at_ns)}
          </p>
        )}
      </div>
    )
  } else if (auth.status !== "signed-in") {
    body = (
      <div className="card empty">
        <h2>Sign in to open the wallet</h2>
        <p className="quiet">
          Its members see the accounts, balances and approvals here once they sign in.
        </p>
        <SignIn />
      </div>
    )
  } else if (me.isError) {
    body = <p className="card error">{failureText(me.error)}</p>
  } else if (!me.isSuccess) {
    body = (
      <div className="card">
        <Reading>Reading who you are to the wallet…</Reading>
      </div>
    )
  } else {
    body = <JoinRequest wallet={id} />
  }
  return (
    <div className="overview">
      <Hero id={id}>
        <p className="chips">
          <span className="chip">Version {status.data.version}</span>
          <span className="chip">
            {formatUnits(status.data.cycles, 12, { maxFractionDigits: 2 })}T cycles
          </span>
        </p>
      </Hero>
      {body}
    </div>
  )
}

function Dashboard({ wallet: id, me }: { wallet: Principal; me: Member }) {
  const { client, wallet } = useWallet(id)
  const accounts = useQuery(client.queryOptions(wallet, "accounts"))
  const assets = useQuery(client.queryOptions(wallet, "assets"))
  // One read per account and asset it shows, shared by the totals and the list.
  const holdings = (accounts.data ?? []).flatMap((account) =>
    assetsOf(account, assets.data ?? ICP_ONLY).map((asset) => ({ account, asset })),
  )
  const reads = useQueries({
    queries: holdings.map(({ account, asset }) =>
      client.queryOptions(wallet, "balance", [account.id, asset], { update: "idempotent" }),
    ),
  })
  const balances: BalanceRead[] = holdings.flatMap((holding, index) => {
    const read = reads[index]
    return read ? [{ ...holding, read }] : []
  })
  return (
    <div className="overview">
      <Hero id={id} me={me}>
        <Holdings accounts={accounts} balances={balances} />
      </Hero>
      <section className="stack" aria-labelledby="open-actions">
        <h2 id="open-actions" className="section-title">
          Open actions
        </h2>
        <OpenActions wallet={id} me={me} />
      </section>
      <Requests wallet={id} me={me} />
      <Accounts accounts={accounts} balances={balances} />
    </div>
  )
}

/** The wallet's mark, name and id, who you are to it, and what it holds. */
function Hero({ id, me, children }: { id: Principal; me?: Member; children?: ReactNode }) {
  return (
    <section className={`hero ${hueClass(id)}`}>
      <div className="hero-head">
        <WalletMark id={id} />
        <div className="hero-name">
          <p className="eyebrow">
            {me ? `${me.role.tag}${me.label ? ` · ${me.label}` : ""}` : "B3Wallet"}
          </p>
          <h1>{me ? "Your wallet" : "Wallet"}</h1>
          <p className="hero-id">
            <code className="principal">{id}</code>
            <CopyButton text={id} label="Copy the wallet's id" />
          </p>
        </div>
      </div>
      {children}
    </section>
  )
}
