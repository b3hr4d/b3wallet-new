import type { Principal } from "@candid-core/schema"
import { useMutation, useQuery } from "@tanstack/react-query"
import { useState } from "react"
import type { Member } from "../generated/wallet.ts"
import { useWallet } from "../ic/canister.ts"
import { failureText, shortText } from "../logic/describe.ts"
import { ShieldIcon } from "./icons.tsx"

/** The largest chunk `guard_upload` takes (`guard::MAX_CHUNK`), under the IC's 2 MiB ingress limit. */
const CHUNK = 1_900_000

const hex = (bytes: Uint8Array) => [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("")

/**
 * The guard's model (decision 0012): which file the wallet loaded, and for
 * owners the upload. The wallet pins the file's SHA-256 in its code, so this
 * page checks the picked file against it before sending anything, then writes
 * it in chunks with `guard_upload` and asks the wallet to load it; the wallet
 * hashes the whole file again and refuses any other.
 */
export function GuardModel({ wallet: id, me }: { wallet: Principal; me: Member }) {
  const { client, wallet } = useWallet(id)
  const status = useQuery(client.queryOptions(wallet, "guard_status"))
  const upload = useMutation(client.mutationOptions(wallet, "guard_upload", { invalidates: [] }))
  const load = useMutation(
    client.mutationOptions(wallet, "guard_load", {
      invalidates: [
        [wallet, "guard_status"],
        [wallet, "audit_log"],
      ],
    }),
  )
  const [file, setFile] = useState<File | undefined>()
  const [problem, setProblem] = useState<string | undefined>()
  const [sent, setSent] = useState<number | undefined>()
  const owner = me.role.tag === "Owner"
  const pinned = status.data && hex(status.data.file_sha256)
  const chunks = status.data ? Math.ceil(Number(status.data.file_bytes) / CHUNK) : 0
  const busy = sent !== undefined || load.isPending

  async function send() {
    if (!file || !status.data || !pinned) return
    setProblem(undefined)
    load.reset()
    upload.reset()
    if (BigInt(file.size) !== status.data.file_bytes) {
      setProblem(
        `This file is ${file.size.toLocaleString()} bytes; this release's guard file is ${Number(status.data.file_bytes).toLocaleString()}.`,
      )
      return
    }
    const bytes = new Uint8Array(await file.arrayBuffer())
    const digest = hex(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)))
    if (digest !== pinned) {
      setProblem(
        `This file's SHA-256 is ${shortText(digest, 8, 8)}, not the one this release pins.`,
      )
      return
    }
    try {
      for (let k = 0; k < chunks; k++) {
        setSent(k)
        await upload.mutateAsync({
          offset: BigInt(k * CHUNK),
          chunk: bytes.subarray(k * CHUNK, (k + 1) * CHUNK),
        })
      }
    } catch {
      setSent(undefined)
      return
    }
    setSent(undefined)
    load.mutate()
  }

  return (
    <section id="guard" className="card">
      <div className="card-head">
        <ShieldIcon />
        <h2>Guard</h2>
      </div>
      <p className="quiet">
        The guard reads what you asked your agent to do and flags a transfer that differs from it.
        Its model runs inside this wallet, from a file owners upload once.
      </p>
      {status.isError ? (
        <p className="error">{failureText(status.error)}</p>
      ) : status.data === undefined ? (
        <span className="skeleton wide" />
      ) : (
        <dl className="settings-rows">
          <div>
            <dt>Model</dt>
            <dd>
              {status.data.loaded ? (
                <p>Loaded: {status.data.release ?? "unnamed release"}</p>
              ) : (
                <p className="error">Not loaded: transfers are not checked</p>
              )}
            </dd>
          </div>
          <div>
            <dt>File this release takes</dt>
            <dd>
              <p className="quiet small">
                {Number(status.data.file_bytes).toLocaleString()} bytes, SHA-256{" "}
                <code title={pinned}>{shortText(pinned ?? "", 8, 8)}</code>
              </p>
            </dd>
          </div>
        </dl>
      )}
      {owner && status.data && !status.data.loaded && (
        <form
          className="form"
          onSubmit={(event) => {
            event.preventDefault()
            void send()
          }}
        >
          <label>
            Guard file (guard.bin)
            <input
              type="file"
              accept=".bin,application/octet-stream"
              disabled={busy}
              onChange={(event) => {
                setFile(event.target.files?.[0])
                setProblem(undefined)
              }}
            />
          </label>
          <div className="buttons">
            <button type="submit" className="primary" disabled={!file || busy}>
              {sent !== undefined
                ? `Uploading ${sent + 1} of ${chunks}…`
                : load.isPending
                  ? "Loading…"
                  : "Upload and load"}
            </button>
          </div>
          <p className="quiet small">
            {chunks} calls of up to 1.9 MB each, then one to load. Each is signed by you, and the
            wallet checks the whole file before it uses it.
          </p>
          {problem && <p className="error">{problem}</p>}
          {upload.isError && <p className="error">{failureText(upload.error)}</p>}
          {load.isError && <p className="error">{failureText(load.error)}</p>}
        </form>
      )}
    </section>
  )
}
