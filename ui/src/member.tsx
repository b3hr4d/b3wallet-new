import type { Principal } from "@candid-core/schema"
import { useAuth } from "@ic-reactor/react"
import { skipToken, useQuery } from "@tanstack/react-query"
import type { ReactNode } from "react"
import { useWallet } from "./canister.ts"
import { failureText } from "./describe.ts"
import type { Member } from "./generated/wallet.ts"
import { Session } from "./session.tsx"

/**
 * The wallet's status, and who the signed-in caller is to it once it is
 * claimed: a member, or `null` for someone who is not one.
 */
export function useMe(id: Principal) {
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
  return { auth, status, claimed, me }
}

/**
 * A page for members: `children` renders for a signed-in member, and anyone
 * else reads what is missing. The sign-in sits below either.
 */
export function AsMember({
  wallet,
  children,
}: {
  wallet: Principal
  children: (me: Member) => ReactNode
}) {
  const { auth, status, me } = useMe(wallet)
  let body: ReactNode
  if (status.isError) {
    body = <p className="error">{failureText(status.error)}</p>
  } else if (status.data === undefined) {
    body = <p className="quiet">Reading the wallet…</p>
  } else if (!status.data.claimed) {
    body = <p>This wallet has no owner yet.</p>
  } else if (auth.status !== "signed-in") {
    body = <p>Sign in to see this page.</p>
  } else if (me.isError) {
    body = <p className="error">{failureText(me.error)}</p>
  } else if (!me.isSuccess) {
    body = <p className="quiet">Reading who you are to the wallet…</p>
  } else if (me.data === null) {
    body = (
      <p>
        You are not a member of this wallet. <a href="/">Ask to join on its start page.</a>
      </p>
    )
  } else {
    body = children(me.data)
  }
  return (
    <>
      {body}
      <Session />
    </>
  )
}
