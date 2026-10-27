import { isPrincipal, type Principal } from "@candid-core/schema"
import { useMutation } from "@tanstack/react-query"
import { useState } from "react"
import type { Member } from "../generated/wallet.ts"
import { useWallet } from "../ic/canister.ts"
import { failureText } from "../logic/describe.ts"
import { hashOf } from "../logic/hash.ts"
import { KeyIcon } from "./icons.tsx"

/**
 * For owners: proposes handing control of the wallet's code to a recovery
 * controller that their recovery keys hold (decision 0009), which can then
 * repair the wallet when its own pages or upgrades break. The hand-over
 * cannot be undone, and waits for the policy-change quorum and timelock.
 * The wallet checks the controller again when it runs: its module, that it
 * has no controller and was installed once, and that it serves this wallet
 * with the named policy.
 */
export function Recovery({ wallet: id, me }: { wallet: Principal; me: Member }) {
  const { client, wallet } = useWallet(id)
  const [open, setOpen] = useState(false)
  const [controller, setController] = useState("")
  const [module, setModule] = useState("")
  const [policy, setPolicy] = useState("")
  const propose = useMutation(
    client.mutationOptions(wallet, "propose", { invalidates: [[wallet, "pending"]] }),
  )
  if (me.role.tag !== "Owner") return null
  const controllerId = controller.trim()
  const moduleHash = hashOf(module)
  const policyHash = hashOf(policy)
  const ready =
    isPrincipal(controllerId) &&
    controllerId !== id &&
    moduleHash !== undefined &&
    policyHash !== undefined
  return (
    <section className="card">
      <div className="card-head">
        <KeyIcon />
        <h2>Recovery controller</h2>
      </div>
      <p className="quiet small">
        A recovery controller held by your own recovery keys can repair this wallet when its pages
        or upgrades break. Handing control to one cannot be undone. Create it and check it with
        recovery-tool first, as docs/recovery.md in the wallet&apos;s source describes.
      </p>
      {propose.isSuccess ? (
        <p>
          The hand-over is action {String(propose.data.id)}.{" "}
          <a href="/approvals">Approve and run it on the Approvals page.</a>
        </p>
      ) : !open ? (
        <div className="buttons">
          <button type="button" className="small" onClick={() => setOpen(true)}>
            Hand control to a recovery controller
          </button>
        </div>
      ) : (
        <form
          className="form"
          onSubmit={(event) => {
            event.preventDefault()
            if (!ready) return
            propose.mutate({
              tag: "HandOverControl",
              value: {
                controller: controllerId,
                controller_module_hash: moduleHash,
                controller_policy_sha256: policyHash,
              },
            })
          }}
        >
          <label>
            Controller id
            <input
              value={controller}
              onChange={(event) => setController(event.target.value)}
              placeholder="The controller canister's principal"
              autoComplete="off"
              spellCheck={false}
            />
          </label>
          <label>
            Its module hash
            <input
              value={module}
              onChange={(event) => setModule(event.target.value)}
              placeholder="64 hex digits"
              autoComplete="off"
              spellCheck={false}
            />
          </label>
          <label>
            Its policy hash
            <input
              value={policy}
              onChange={(event) => setPolicy(event.target.value)}
              placeholder="64 hex digits, as recovery-tool policy prints it"
              autoComplete="off"
              spellCheck={false}
            />
          </label>
          <div className="buttons">
            <button type="submit" className="primary small" disabled={!ready || propose.isPending}>
              {propose.isPending ? "Proposing…" : "Propose the hand-over"}
            </button>
            <button type="button" className="small" onClick={() => setOpen(false)}>
              Cancel
            </button>
          </div>
          {propose.isError && <p className="error">{failureText(propose.error)}</p>}
        </form>
      )}
    </section>
  )
}
