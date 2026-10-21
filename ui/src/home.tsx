import type { Principal } from "@candid-core/schema"
import { formatUnits } from "@ic-reactor/core"
import { Accounts } from "./accounts.tsx"
import { failureText, roleText } from "./describe.ts"
import type { UpgradeState } from "./generated/wallet.ts"
import { JoinRequest } from "./join.tsx"
import { useMe } from "./member.tsx"
import { Release } from "./release.tsx"
import { Session } from "./session.tsx"
import { dateOf } from "./time.ts"

/**
 * The wallet's start page: what it runs and who you are to it, then for a
 * member its accounts and new versions, and for anyone else signed in a
 * request to join.
 */
export function Home({ wallet: id, local }: { wallet: Principal; local: boolean }) {
  const { auth, status, claimed, me } = useMe(id)

  return (
    <section>
      <h1>Wallet</h1>
      <p>
        <code className="principal">{id}</code>
      </p>
      {status.isError ? (
        <p className="error">{failureText(status.error)}</p>
      ) : status.data === undefined ? (
        <p className="quiet">Reading the wallet…</p>
      ) : (
        <>
          <dl className="facts">
            <dt>Version</dt>
            <dd>{status.data.version}</dd>
            <dt>Cycles</dt>
            <dd>{formatUnits(status.data.cycles, 12, { maxFractionDigits: 3 })} T</dd>
          </dl>
          {upgradeText(status.data.upgrade) && <p>{upgradeText(status.data.upgrade)}</p>}
          {!claimed && (
            <p>
              This wallet has no owner yet. Whoever opens its claim link and signs in becomes its
              first owner.
            </p>
          )}
        </>
      )}
      {auth.status === "signed-in" &&
        claimed &&
        (me.isError ? (
          <p className="error">{failureText(me.error)}</p>
        ) : me.isSuccess ? (
          me.data === null ? (
            <JoinRequest wallet={id} />
          ) : (
            <>
              <p>
                You are {roleText(me.data.role)} of this wallet
                {me.data.label ? `, as “${me.data.label}”` : ""}.
              </p>
              <Accounts wallet={id} me={me.data} local={local} />
              <Release wallet={id} me={me.data} />
            </>
          )
        ) : (
          <p className="quiet">Reading who you are to the wallet…</p>
        ))}
      <Session />
    </section>
  )
}

function upgradeText(upgrade: UpgradeState): string | undefined {
  switch (upgrade.tag) {
    case "Idle":
      return undefined
    case "Staging":
      return `Staging version ${upgrade.value.version}: ${upgrade.value.chunks_uploaded} of ${upgrade.value.chunks_total} chunks.`
    case "Installing":
      return `Installing version ${upgrade.value.version}.`
    case "Failed":
      return `The upgrade to version ${upgrade.value.version} failed on ${dateOf(upgrade.value.at_ns)}: ${upgrade.value.reason}`
  }
}
