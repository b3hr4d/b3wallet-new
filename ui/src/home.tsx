import type { Principal } from "@candid-core/schema"
import { formatUnits } from "@ic-reactor/core"
import { useAuth } from "@ic-reactor/react"
import { skipToken, useQuery } from "@tanstack/react-query"
import { dateOf, useWallet } from "./canister.ts"
import { failureText } from "./describe.ts"
import type { Role, UpgradeState } from "./generated/wallet.ts"
import { Session } from "./session.tsx"

/** The wallet's start page: what it runs, and who you are to it. */
export function Home({ wallet: id }: { wallet: Principal }) {
  const auth = useAuth()
  const { client, wallet } = useWallet(id)
  const status = useQuery(client.queryOptions(wallet, "status"))
  const claimed = status.data?.claimed === true
  const me = useQuery(
    client.queryOptions(
      wallet,
      "whoami",
      auth.status === "signed-in" && claimed ? undefined : skipToken,
    ),
  )

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
      <Session />
      {auth.status === "signed-in" &&
        claimed &&
        (me.isError ? (
          <p className="error">{failureText(me.error)}</p>
        ) : me.isSuccess ? (
          me.data === null ? (
            <p>You are not a member of this wallet. A member can add you.</p>
          ) : (
            <p>
              You are {roleText(me.data.role)} of this wallet
              {me.data.label ? `, as “${me.data.label}”` : ""}.
            </p>
          )
        ) : (
          <p className="quiet">Reading who you are to the wallet…</p>
        ))}
    </section>
  )
}

function roleText(role: Role): string {
  switch (role.tag) {
    case "Owner":
      return "an owner"
    case "Approver":
      return "an approver"
    case "Viewer":
      return "a viewer"
  }
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
