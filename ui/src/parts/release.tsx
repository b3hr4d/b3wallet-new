import type { Principal } from "@candid-core/schema"
import { useMutation } from "@tanstack/react-query"
import type { Member, ReleaseManifest } from "../generated/wallet.ts"
import { useWallet } from "../ic/canister.ts"
import { failureText } from "../logic/describe.ts"
import { dateOf } from "../logic/time.ts"
import { CheckIcon } from "./icons.tsx"

/**
 * A button that asks whether the wallet's release channel has a release it
 * would install, which an owner may then propose. The wallet checks the
 * signatures and that the release was tested from the version it runs; it
 * checks them all again before it installs.
 */
export function ReleaseCheck({ wallet: id, me }: { wallet: Principal; me: Member }) {
  const { client, wallet } = useWallet(id)
  const check = useMutation(
    client.mutationOptions(wallet, "release_available", { invalidates: [] }),
  )
  return (
    <>
      {check.isSuccess ? (
        check.data === null ? (
          <p className="tile-note good">
            <CheckIcon />
            The latest release
          </p>
        ) : (
          <Available wallet={id} release={check.data} owner={me.role.tag === "Owner"} />
        )
      ) : (
        <div className="tile-note">
          <button
            type="button"
            className="small"
            disabled={check.isPending}
            onClick={() => check.mutate()}
          >
            {check.isPending ? "Asking…" : "Check for a new version"}
          </button>
        </div>
      )}
      {check.isError && <p className="error">{failureText(check.error)}</p>}
    </>
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
    <div className="tile-note release">
      <p>
        <strong>Version {release.version}</strong> is available, published{" "}
        {dateOf(release.published_at_ns)}.
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
        <button
          type="button"
          className="primary small"
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
      ) : (
        <p>An owner proposes the upgrade.</p>
      )}
      {propose.isError && <p className="error">{failureText(propose.error)}</p>}
    </div>
  )
}
