import type { Principal } from "@candid-core/schema"
import { useMutation } from "@tanstack/react-query"
import { useWallet } from "./canister.ts"
import { failureText } from "./describe.ts"
import type { Member, ReleaseManifest } from "./generated/wallet.ts"
import { dateOf } from "./time.ts"

/**
 * Asks the wallet whether its release channel has a release it would
 * install, and lets an owner propose the upgrade. The wallet checks the
 * signatures and that the release was tested from the version it runs; it
 * checks them all again before it installs.
 */
export function Release({ wallet: id, me }: { wallet: Principal; me: Member }) {
  const { client, wallet } = useWallet(id)
  const check = useMutation(
    client.mutationOptions(wallet, "release_available", { invalidates: [] }),
  )
  return (
    <section className="block">
      <h2>New versions</h2>
      {check.isSuccess ? (
        check.data === null ? (
          <p>The wallet runs the latest release.</p>
        ) : (
          <Available wallet={id} release={check.data} owner={me.role.tag === "Owner"} />
        )
      ) : (
        <p>The wallet installs a new version only when its owners approve it.</p>
      )}
      <div className="buttons">
        <button type="button" disabled={check.isPending} onClick={() => check.mutate()}>
          {check.isPending ? "Asking…" : "Check for a new version"}
        </button>
      </div>
      {check.isError && <p className="error">{failureText(check.error)}</p>}
    </section>
  )
}

function Available({
  wallet: id,
  release,
  owner,
}: {
  wallet: Principal
  release: ReleaseManifest
  owner: boolean
}) {
  const { client, wallet } = useWallet(id)
  const propose = useMutation(
    client.mutationOptions(wallet, "propose", { invalidates: [[wallet, "pending"]] }),
  )
  return (
    <>
      <p>
        Version {release.version} is available, published {dateOf(release.published_at_ns)}.
        {release.notes_url.startsWith("https://") && (
          <>
            {" "}
            <a href={release.notes_url} rel="noreferrer">
              What it changes
            </a>
          </>
        )}
      </p>
      {propose.isSuccess ? (
        <p>
          The upgrade is action {String(propose.data.id)}.{" "}
          <a href="/approvals">Approve and run it on the Approvals page.</a>
        </p>
      ) : owner ? (
        <div className="buttons">
          <button
            type="button"
            className="primary"
            disabled={propose.isPending}
            onClick={() =>
              propose.mutate({
                tag: "Upgrade",
                value: { version: release.version, wasm_sha256: release.wasm_sha256 },
              })
            }
          >
            {propose.isPending ? "Proposing…" : "Propose the upgrade"}
          </button>
        </div>
      ) : (
        <p>An owner proposes the upgrade.</p>
      )}
      {propose.isError && <p className="error">{failureText(propose.error)}</p>}
    </>
  )
}
