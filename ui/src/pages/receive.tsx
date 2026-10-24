import type { Principal } from "@candid-core/schema"
import { useQuery } from "@tanstack/react-query"
import { useMemo, useState } from "react"
import type { Account } from "../generated/wallet.ts"
import { useWallet } from "../ic/canister.ts"
import { addressGroups, failureText, receiveNote, type AddressForm } from "../logic/describe.ts"
import { numberParam } from "../logic/host.ts"
import { qrOf } from "../logic/qr.ts"
import { kindText } from "../logic/tokens.ts"
import { KindAvatar } from "../parts/accounts.tsx"
import { CopyButton } from "../parts/copy.tsx"
import { AsMember, Reading } from "../parts/member.tsx"

/**
 * The receive page: an account's whole address, to give whoever sends to it.
 * Anyone may send to an address; only the wallet's owners move what arrives,
 * through its policy. `?account=<id>` opens it on that account.
 */
export function Receive({ wallet }: { wallet: Principal }) {
  return (
    <section className="column">
      <div className="page-head">
        <p className="eyebrow">Your addresses</p>
        <h1>Receive</h1>
        <p className="quiet">
          Give the sender the whole address of the account that should receive, and check it with
          them character by character, not only its ends.
        </p>
      </div>
      <AsMember wallet={wallet}>{() => <Accounts wallet={wallet} />}</AsMember>
    </section>
  )
}

function Accounts({ wallet: id }: { wallet: Principal }) {
  const { client, wallet } = useWallet(id)
  const accounts = useQuery(client.queryOptions(wallet, "accounts"))
  const [picked, setPicked] = useState(() => numberParam(window.location.search, "account"))
  if (accounts.isError) return <p className="card error">{failureText(accounts.error)}</p>
  if (accounts.data === undefined) {
    return (
      <div className="card">
        <Reading>Reading the accounts…</Reading>
      </div>
    )
  }
  const account = accounts.data.find((candidate) => candidate.id === picked) ?? accounts.data[0]
  if (account === undefined) {
    return <p className="card">This wallet has no accounts yet.</p>
  }
  return (
    <div className="receive">
      {accounts.data.length > 1 && (
        <fieldset className="choice">
          <legend>Account</legend>
          <div className="picks">
            {accounts.data.map((candidate) => (
              <label key={candidate.id} className="pick">
                <input
                  type="radio"
                  name="account"
                  checked={candidate.id === account.id}
                  onChange={() => setPicked(candidate.id)}
                />
                <span className="pick-body">
                  <KindAvatar kind={candidate.kind} />
                  <span className="pick-name">
                    <strong>{candidate.label}</strong>
                    <span>{kindText(candidate.kind)}</span>
                  </span>
                </span>
              </label>
            ))}
          </div>
        </fieldset>
      )}
      <Address key={account.id} account={account} />
    </div>
  )
}

/**
 * The address itself, large and in groups, its QR code beside it, with what
 * a sender must know. An Internet Computer account has two forms: its ICRC-1
 * account, for ICP and every ICRC-1 token, and its ICP account identifier,
 * for exchanges that send ICP only to that. The QR code holds exactly the
 * form the page shows and copies.
 */
function Address({ account }: { account: Account }) {
  const [form, setForm] = useState<AddressForm>("account")
  const text =
    form === "identifier" && account.icp_account_id !== null
      ? account.icp_account_id
      : account.address
  const qr = useMemo(() => qrOf(text), [text])
  return (
    <div className="card receive-card">
      <div className="receive-head">
        <KindAvatar kind={account.kind} />
        <div>
          <h2>{account.label}</h2>
          <p className="quiet small">{kindText(account.kind)}</p>
        </div>
      </div>
      {account.icp_account_id !== null && (
        <fieldset className="choice">
          <legend>Form</legend>
          <div className="segmented">
            <label>
              <input
                type="radio"
                name="form"
                checked={form === "account"}
                onChange={() => setForm("account")}
              />
              <span>ICRC-1 account</span>
            </label>
            <label>
              <input
                type="radio"
                name="form"
                checked={form === "identifier"}
                onChange={() => setForm("identifier")}
              />
              <span>Account identifier, for exchanges</span>
            </label>
          </div>
        </fieldset>
      )}
      <div className="receive-body">
        <figure className="qr">
          <svg
            viewBox={`0 0 ${qr.size} ${qr.size}`}
            width="200"
            height="200"
            shapeRendering="crispEdges"
            role="img"
            aria-label={`QR code of the address of ${account.label}`}
          >
            <rect className="qr-light" width={qr.size} height={qr.size} />
            <path className="qr-dark" d={qr.path} />
          </svg>
          <figcaption>Scan to send</figcaption>
        </figure>
        <div className="receive-main">
          <div className="receive-address">
            <p className="field-label">
              {form === "identifier" ? "ICP account identifier" : "Address"}
            </p>
            <p className="receive-text">
              <code>
                {addressGroups(text).map((group, index) => (
                  <span key={index}>{group}</span>
                ))}
              </code>
            </p>
          </div>
          <div className="buttons">
            <CopyButton text={text} name="Copy the address" primary />
          </div>
          <p className="receive-note">{receiveNote(account.kind, form)}</p>
        </div>
      </div>
    </div>
  )
}
