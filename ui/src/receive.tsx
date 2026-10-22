import type { Principal } from "@candid-core/schema"
import { useQuery } from "@tanstack/react-query"
import { useMemo, useState } from "react"
import { chainName, TokenAvatar } from "./accounts.tsx"
import { useWallet } from "./canister.ts"
import { CopyButton } from "./copy.tsx"
import { addressGroups, failureText, receiveNote } from "./describe.ts"
import type { Account } from "./generated/wallet.ts"
import { AsMember, Reading } from "./member.tsx"
import { qrOf } from "./qr.ts"
import { useToken } from "./tokens.ts"

/**
 * The receive page: an account's whole address, to give whoever sends to it.
 * Anyone may send to an address; only the wallet's owners move what arrives,
 * through its policy. `?account=<id>` opens it on that account.
 */
export function Receive({ wallet }: { wallet: Principal }) {
  return (
    <section className="column">
      <div className="page-head">
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

/** The account the address names, as a number, if it names one. */
function accountInAddress(): number | undefined {
  const id = Number(new URLSearchParams(window.location.search).get("account") ?? "")
  return Number.isInteger(id) && id > 0 ? id : undefined
}

function Accounts({ wallet: id }: { wallet: Principal }) {
  const { client, wallet } = useWallet(id)
  const accounts = useQuery(client.queryOptions(wallet, "accounts"))
  const [picked, setPicked] = useState(accountInAddress)
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
    return (
      <p className="card">
        This wallet has no accounts yet. An owner adds one on <a href="/">its start page</a>.
      </p>
    )
  }
  return (
    <div className="receive">
      {accounts.data.length > 1 && (
        <fieldset className="choice">
          <legend>Account</legend>
          <div className="picks">
            {accounts.data.map((candidate) => (
              <Pick
                key={candidate.id}
                account={candidate}
                checked={candidate.id === account.id}
                onPick={() => setPicked(candidate.id)}
              />
            ))}
          </div>
        </fieldset>
      )}
      <Address account={account} />
    </div>
  )
}

function Pick({
  account,
  checked,
  onPick,
}: {
  account: Account
  checked: boolean
  onPick: () => void
}) {
  const { token } = useToken(account.chain)
  return (
    <label className="pick">
      <input type="radio" name="account" checked={checked} onChange={onPick} />
      <span className="pick-body">
        <TokenAvatar chain={account.chain} token={token} />
        <span className="pick-name">
          <strong>{account.label}</strong>
          <span>{chainName(account.chain, token)}</span>
        </span>
      </span>
    </label>
  )
}

/**
 * The address itself, large and in groups, its QR code beside it, with what
 * a sender must know. The QR code holds exactly the address the page shows
 * and copies.
 */
function Address({ account }: { account: Account }) {
  const { token } = useToken(account.chain)
  const qr = useMemo(() => qrOf(account.address), [account.address])
  return (
    <div className="card receive-card">
      <div className="receive-head">
        <TokenAvatar chain={account.chain} token={token} />
        <div>
          <h2>{account.label}</h2>
          <p className="quiet small">{chainName(account.chain, token)}</p>
        </div>
      </div>
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
            <p className="field-label">Address</p>
            <p className="receive-text">
              <code>
                {addressGroups(account.address).map((group, index) => (
                  <span key={index}>{group}</span>
                ))}
              </code>
            </p>
          </div>
          <div className="buttons">
            <CopyButton text={account.address} name="Copy the address" primary />
          </div>
          <p className="receive-note">{receiveNote(account.chain, token)}</p>
        </div>
      </div>
    </div>
  )
}
