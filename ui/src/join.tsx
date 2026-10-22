import type { Principal } from "@candid-core/schema"
import { useMutation } from "@tanstack/react-query"
import { useState } from "react"
import { useWallet } from "./canister.ts"
import { failureText } from "./describe.ts"
import { joinCode } from "./join-code.ts"
import { dateOf } from "./time.ts"

/**
 * For someone signed in who is not a member: asks to join as a viewer. The
 * request is an action the members approve, open for an hour; asking again
 * while it is open returns the same request.
 */
export function JoinRequest({ wallet: id }: { wallet: Principal }) {
  const { client, wallet } = useWallet(id)
  const join = useMutation(client.mutationOptions(wallet, "request_join", { invalidates: [] }))
  const [label, setLabel] = useState("")

  if (join.isSuccess) {
    return (
      <section className="block">
        <h2>Your request to join</h2>
        <p>An owner approves it on the wallet&apos;s Approvals page, where it shows the code</p>
        <p className="code">{joinCode(join.data.proposed_by)}</p>
        <p>
          Check with them that it is this code, so they approve this device and no other. The
          request is open until {dateOf(join.data.expires_at_ns)}. Once it is approved and run, open
          this page again.
        </p>
      </section>
    )
  }
  return (
    <form
      className="form block"
      onSubmit={(event) => {
        event.preventDefault()
        if (label.trim()) join.mutate([label.trim(), null])
      }}
    >
      <p>
        You are not a member of this wallet. Ask to join as a viewer, and an owner approves the
        request.
      </p>
      <label>
        A name for this device
        <input
          value={label}
          onChange={(event) => setLabel(event.target.value)}
          placeholder="Laptop"
          autoComplete="off"
        />
      </label>
      <div className="buttons">
        <button type="submit" className="primary" disabled={!label.trim() || join.isPending}>
          {join.isPending ? "Asking…" : "Ask to join"}
        </button>
      </div>
      {join.isError && <p className="error">{failureText(join.error)}</p>}
    </form>
  )
}
