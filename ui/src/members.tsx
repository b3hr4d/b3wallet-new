import { isPrincipal, principal, type Principal } from "@candid-core/schema"
import { useMutation, useQuery } from "@tanstack/react-query"
import { useMemo, useState } from "react"
import { StateChip } from "./approvals.tsx"
import { useWallet } from "./canister.ts"
import { CopyButton } from "./copy.tsx"
import {
  actionText,
  canRun,
  failureText,
  roleText,
  shortText,
  stateText,
  whoText,
} from "./describe.ts"
import type { Member, Pending, Policy, Role } from "./generated/wallet.ts"
import { changeRefusal, labelRefusal } from "./governance.ts"
import { AlertIcon, PeopleIcon, PlusIcon } from "./icons.tsx"
import { initialOf } from "./mark.ts"
import { AsMember, Reading } from "./member.tsx"
import { useNowNs } from "./now.ts"
import { byRole, CHIP } from "./people.tsx"
import { qrOf } from "./qr.ts"
import { dateOf } from "./time.ts"

const ROLES: readonly Role["tag"][] = ["Owner", "Approver", "Proposer", "Viewer"]

/** What each role may do, as the wallet's guards and policy allow it. */
const ROLE_TEXT: Record<Role["tag"], string> = {
  Owner: "Proposes, approves, runs and rejects every action.",
  Approver: "Approves and runs actions; proposes nothing.",
  Proposer: "Reads, and proposes transfers that always wait for an owner or an approver.",
  Viewer: "Reads the wallet; changes nothing.",
}

/**
 * The members page: who can see and move the wallet, and for owners the
 * changes to that. Each change is an action that waits for the approvals the
 * policy asks for, so a stolen session cannot quietly remove the other
 * owners. Nothing can recover a wallet whose last owner is lost, so the page
 * says so while there is one.
 */
export function MembersPage({ wallet }: { wallet: Principal }) {
  return (
    <section className="column">
      <p className="back">
        <a href="/settings">Settings</a>
      </p>
      <div className="page-head">
        <h1>Members</h1>
        <p className="quiet">
          Who can see and move this wallet, and with what role. Every change waits for the approvals
          the policy asks for.
        </p>
      </div>
      <AsMember wallet={wallet}>{(me) => <Manage wallet={wallet} me={me} />}</AsMember>
    </section>
  )
}

function Manage({ wallet: id, me }: { wallet: Principal; me: Member }) {
  const { client, wallet } = useWallet(id)
  const members = useQuery(client.queryOptions(wallet, "members"))
  const policy = useQuery(client.queryOptions(wallet, "policy"))
  const pending = useQuery(client.queryOptions(wallet, "pending"))
  const [proposed, setProposed] = useState<Pending>()
  const owner = me.role.tag === "Owner"
  if (members.isError) return <p className="card error">{failureText(members.error)}</p>
  if (members.data === undefined) {
    return (
      <div className="card">
        <Reading>Reading the members…</Reading>
      </div>
    )
  }
  const all = members.data
  const owners = all.filter((member) => member.role.tag === "Owner").length
  return (
    <div className="stack">
      {owners === 1 && (
        <div className="notice-card warn">
          <AlertIcon />
          <div className="notice-body">
            <p className="notice-title">This wallet has one owner</p>
            <p>
              If that device or its Internet Identity is lost, nothing can recover the wallet. Add a
              second device, then make it an owner.
            </p>
          </div>
        </div>
      )}
      {proposed && (
        <Proposed wallet={id} pending={proposed} onDone={() => setProposed(undefined)} />
      )}
      <section className="card">
        <div className="card-head">
          <PeopleIcon />
          <h2>Members</h2>
          <span className="count">{all.length}</span>
        </div>
        <ul className="roster">
          {[...all].sort(byRole).map((member) => (
            <MemberItem
              key={member.id}
              wallet={id}
              member={member}
              me={me}
              members={all}
              policy={policy.data}
              pending={pending.data}
              onProposed={setProposed}
            />
          ))}
        </ul>
      </section>
      {owner && <AddMember wallet={id} members={all} onProposed={setProposed} />}
      <section className="card">
        <h2>Roles</h2>
        <dl className="roles">
          {ROLES.map((role) => (
            <div key={role}>
              <dt>
                <span className={CHIP[role]}>{role}</span>
              </dt>
              <dd className="quiet">{ROLE_TEXT[role]}</dd>
            </div>
          ))}
        </dl>
      </section>
    </div>
  )
}

/**
 * One member, with the changes an owner may propose to them: another role,
 * or removal. The page refuses what the wallet would refuse, and says why.
 */
function MemberItem({
  wallet: id,
  member,
  me,
  members,
  policy,
  pending,
  onProposed,
}: {
  wallet: Principal
  member: Member
  me: Member
  members: readonly Member[]
  policy: Policy | undefined
  pending: Pending[] | undefined
  onProposed: (pending: Pending) => void
}) {
  const { client, wallet } = useWallet(id)
  const propose = useMutation(
    client.mutationOptions(wallet, "propose", { invalidates: [[wallet, "pending"]] }),
  )
  const [step, setStep] = useState<"idle" | "role" | "remove">("idle")
  const [role, setRole] = useState<Role["tag"]>(member.role.tag)
  const owner = me.role.tag === "Owner"
  const you = member.id === me.id
  const open = (pending ?? []).filter(
    (action) =>
      (action.action.tag === "RemoveMember" && action.action.value === member.id) ||
      (action.action.tag === "ChangeRole" && action.action.value.id === member.id),
  )
  const removal = changeRefusal(members, policy, { remove: member.id })
  const change =
    role === member.role.tag
      ? `${member.label} is ${roleText(member.role)} already.`
      : changeRefusal(members, policy, { id: member.id, role: { tag: role } })
  const send = (action: Parameters<typeof propose.mutate>[0]) =>
    propose.mutate(action, {
      onSuccess: (result) => {
        setStep("idle")
        onProposed(result)
      },
    })

  return (
    <li className="roster-item">
      <div className="roster-head">
        <span className="avatar" aria-hidden="true">
          {initialOf(member.label)}
        </span>
        <div className="roster-name">
          <p>
            <strong>{member.label || shortText(member.id)}</strong>
            {you && <span className="quiet"> · you</span>}
          </p>
          <p className="quiet small">
            Since {dateOf(member.added_at_ns)},{" "}
            {member.added_by === null
              ? "claimed the wallet"
              : `added by ${member.added_by === me.id ? "you" : whoText(member.added_by, me, members)}`}
          </p>
        </div>
        <span className={CHIP[member.role.tag]}>{member.role.tag}</span>
      </div>
      <p className="address-box">
        <code>{member.id}</code>
        <CopyButton text={member.id} label={`Copy the principal of ${member.label}`} />
      </p>
      {open.map((action) => (
        <p key={String(action.id)} className="roster-open">
          <span className="pill pending">
            <span className="pill-mark" aria-hidden="true" />
            Open
          </span>
          <a href="/approvals">
            Action {String(action.id)}: {actionText(action.action, undefined, undefined)}
          </a>
        </p>
      ))}
      {owner && step === "idle" && (
        <div className="buttons">
          <button type="button" className="small" onClick={() => setStep("role")}>
            Change role
          </button>
          <button type="button" className="small danger" onClick={() => setStep("remove")}>
            Remove
          </button>
        </div>
      )}
      {owner && step === "role" && (
        <form
          className="panel form"
          onSubmit={(event) => {
            event.preventDefault()
            if (change === undefined) {
              send({ tag: "ChangeRole", value: { id: member.id, role: { tag: role } } })
            }
          }}
        >
          <fieldset className="choice">
            <legend>New role</legend>
            <div className="segmented">
              {ROLES.map((candidate) => (
                <label key={candidate}>
                  <input
                    type="radio"
                    name={`role-${member.id}`}
                    checked={role === candidate}
                    onChange={() => setRole(candidate)}
                  />
                  <span>{candidate}</span>
                </label>
              ))}
            </div>
          </fieldset>
          <p className="quiet small">{ROLE_TEXT[role]}</p>
          {change !== undefined && role !== member.role.tag && (
            <p className="error small">{change}</p>
          )}
          <div className="buttons">
            <button
              type="submit"
              className="primary small"
              disabled={change !== undefined || propose.isPending}
            >
              {propose.isPending ? "Proposing…" : "Propose the change"}
            </button>
            <button type="button" className="ghost small" onClick={() => setStep("idle")}>
              Cancel
            </button>
          </div>
          {propose.isError && <p className="error small">{failureText(propose.error)}</p>}
        </form>
      )}
      {owner && step === "remove" && (
        <div className="panel form">
          <p>
            Remove <strong>{member.label}</strong>? It can then no longer read the wallet or approve
            anything.
            {you && " This is you: this device loses its access."}
          </p>
          {removal !== undefined && <p className="error small">{removal}</p>}
          <div className="buttons">
            <button
              type="button"
              className="small danger"
              disabled={removal !== undefined || propose.isPending}
              onClick={() => send({ tag: "RemoveMember", value: member.id })}
            >
              {propose.isPending ? "Proposing…" : "Propose the removal"}
            </button>
            <button type="button" className="ghost small" onClick={() => setStep("idle")}>
              Cancel
            </button>
          </div>
          {propose.isError && <p className="error small">{failureText(propose.error)}</p>}
        </div>
      )}
    </li>
  )
}

/**
 * Adds a device or a person: from the new device, which asks to join on the
 * wallet's own page and shows a code to compare, or by its principal. Internet
 * Identity gives every site its own principal, so the one to add is the one
 * the device shows signed in on this wallet's page.
 */
function AddMember({
  wallet: id,
  members,
  onProposed,
}: {
  wallet: Principal
  members: readonly Member[]
  onProposed: (pending: Pending) => void
}) {
  const { client, wallet } = useWallet(id)
  const propose = useMutation(
    client.mutationOptions(wallet, "propose", { invalidates: [[wallet, "pending"]] }),
  )
  const [way, setWay] = useState<"device" | "principal">("device")
  const [text, setText] = useState("")
  const [label, setLabel] = useState("")
  const [role, setRole] = useState<Role["tag"]>("Viewer")
  const address = `${window.location.origin}/`
  const qr = useMemo(() => qrOf(address), [address])
  const typed = text.trim()
  const known = members.some((member) => member.id === typed)
  const refusal =
    typed === ""
      ? "Paste its principal."
      : !isPrincipal(typed)
        ? "That is not a principal."
        : known
          ? "That principal is a member already."
          : members.length >= 64
            ? "A wallet has at most 64 members."
            : labelRefusal(label)
  return (
    <section className="card">
      <div className="card-head">
        <PlusIcon />
        <h2>Add a device or a person</h2>
      </div>
      <div className="form">
        <fieldset className="choice">
          <legend>How</legend>
          <div className="segmented">
            <label>
              <input
                type="radio"
                name="way"
                checked={way === "device"}
                onChange={() => setWay("device")}
              />
              <span>From the new device</span>
            </label>
            <label>
              <input
                type="radio"
                name="way"
                checked={way === "principal"}
                onChange={() => setWay("principal")}
              />
              <span>By its principal</span>
            </label>
          </div>
        </fieldset>
        {way === "device" ? (
          <div className="add-device">
            <figure className="qr">
              <svg
                viewBox={`0 0 ${qr.size} ${qr.size}`}
                width="168"
                height="168"
                shapeRendering="crispEdges"
                role="img"
                aria-label="QR code of this wallet's address"
              >
                <rect className="qr-light" width={qr.size} height={qr.size} />
                <path className="qr-dark" d={qr.path} />
              </svg>
              <figcaption>Scan on the new device</figcaption>
            </figure>
            <ol className="steps">
              <li className="step">
                <span className="step-number">1</span>
                <div>
                  <p>Open this wallet&apos;s address on the new device.</p>
                  <p className="address-box">
                    <code>{address}</code>
                    <CopyButton text={address} label="Copy the wallet's address" />
                  </p>
                </div>
              </li>
              <li className="step">
                <span className="step-number">2</span>
                <p>Sign in there with Internet Identity, and ask to join. It joins as a viewer.</p>
              </li>
              <li className="step">
                <span className="step-number">3</span>
                <p>
                  Approve it on the <a href="/approvals">Approvals page</a> only if its code matches
                  the one the device shows, then give it its role here.
                </p>
              </li>
            </ol>
          </div>
        ) : (
          <form
            className="form"
            onSubmit={(event) => {
              event.preventDefault()
              if (refusal !== undefined) return
              propose.mutate(
                {
                  tag: "AddMember",
                  value: { id: principal(typed), role: { tag: role }, label: label.trim() },
                },
                {
                  onSuccess: (result) => {
                    setText("")
                    setLabel("")
                    onProposed(result)
                  },
                },
              )
            }}
          >
            <label>
              Principal
              <input
                value={text}
                onChange={(event) => setText(event.target.value)}
                autoComplete="off"
                spellCheck={false}
              />
            </label>
            <p className="quiet small">
              The one the device or person sees at the top of this wallet&apos;s page once signed
              in. Internet Identity gives every site its own.
            </p>
            <label>
              Name
              <input
                value={label}
                onChange={(event) => setLabel(event.target.value)}
                placeholder="Phone"
                autoComplete="off"
              />
            </label>
            <fieldset className="choice">
              <legend>Role</legend>
              <div className="segmented">
                {ROLES.map((candidate) => (
                  <label key={candidate}>
                    <input
                      type="radio"
                      name="new-role"
                      checked={role === candidate}
                      onChange={() => setRole(candidate)}
                    />
                    <span>{candidate}</span>
                  </label>
                ))}
              </div>
            </fieldset>
            <p className="quiet small">{ROLE_TEXT[role]}</p>
            {typed !== "" && refusal !== undefined && <p className="error small">{refusal}</p>}
            <div className="buttons">
              <button
                type="submit"
                className="primary"
                disabled={refusal !== undefined || propose.isPending}
              >
                {propose.isPending ? "Proposing…" : "Propose adding them"}
              </button>
            </div>
            {propose.isError && <p className="error small">{failureText(propose.error)}</p>}
          </form>
        )}
      </div>
    </section>
  )
}

/**
 * A change just proposed: where it stands, and the button that runs it once
 * the policy lets it, as the send page does for a transfer.
 */
function Proposed({
  wallet: id,
  pending,
  onDone,
}: {
  wallet: Principal
  pending: Pending
  onDone: () => void
}) {
  const { client, wallet } = useWallet(id)
  // Running a member change changes the members, the open actions and the
  // health the bar shows, so every read of the wallet is read again.
  const execute = useMutation(client.mutationOptions(wallet, "execute"))
  const now = useNowNs()
  const current = execute.data ?? pending
  return (
    <div className="card">
      <p className="item-head">
        <strong>Action {String(current.id)}</strong>
        <StateChip pending={current} now={now} />
      </p>
      <p>{actionText(current.action, undefined, undefined)}.</p>
      <p className="quiet">{stateText(current, now)}</p>
      {execute.isError && <p className="error">{failureText(execute.error)}</p>}
      <div className="buttons">
        {canRun(current, now) && (
          <button
            type="button"
            className="primary"
            disabled={execute.isPending}
            onClick={() => execute.mutate(current.id)}
          >
            {execute.isPending ? "Running…" : "Run it now"}
          </button>
        )}
        {current.state.tag === "Waiting" && (
          <a className="button" href="/approvals">
            Open Approvals
          </a>
        )}
        <button type="button" className="ghost" onClick={onDone}>
          Done
        </button>
      </div>
    </div>
  )
}
