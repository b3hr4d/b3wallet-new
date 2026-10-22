import type { Principal } from "@candid-core/schema"
import { useQuery, type UseQueryResult } from "@tanstack/react-query"
import { useWallet } from "./canister.ts"
import { failureText, shortText } from "./describe.ts"
import type { ActionKind, Member, Role } from "./generated/wallet.ts"
import { PeopleIcon, ShieldIcon } from "./icons.tsx"
import { hueClass, initialOf } from "./mark.ts"
import { durationText } from "./time.ts"
import { chainAmountText } from "./tokens.ts"

const RANK: Record<Role["tag"], number> = { Owner: 0, Approver: 1, Proposer: 2, Viewer: 3 }

/** A role's chip: owners in the accent, and a proposer, such as an AI agent, apart. */
const CHIP: Record<Role["tag"], string> = {
  Owner: "chip accent",
  Approver: "chip",
  Proposer: "chip warn",
  Viewer: "chip",
}

/** Who the wallet's members are, owners first. */
export function Members({ me, members }: { me: Member; members: UseQueryResult<Member[]> }) {
  const sorted = [...(members.data ?? [])].sort(
    (a, b) => RANK[a.role.tag] - RANK[b.role.tag] || Number(a.added_at_ns - b.added_at_ns),
  )
  return (
    <section className="card">
      <div className="card-head">
        <PeopleIcon />
        <h2>Members</h2>
        {members.data !== undefined && <span className="count">{members.data.length}</span>}
      </div>
      {members.isError ? (
        <p className="error">{failureText(members.error)}</p>
      ) : members.data === undefined ? (
        <ul className="people">
          <li className="person">
            <span className="avatar skeleton" />
            <span className="skeleton wide" />
          </li>
        </ul>
      ) : (
        <ul className="people">
          {sorted.map((member) => (
            <li key={member.id} className="person" title={member.id}>
              <span className={`avatar ${hueClass(member.id)}`} aria-hidden="true">
                {initialOf(member.label)}
              </span>
              <span className="person-name">
                {member.label || shortText(member.id)}
                {member.id === me.id && <span className="quiet"> · you</span>}
              </span>
              <span className={CHIP[member.role.tag]}>{member.role.tag}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  )
}

/** The rules in the order the page lists them, the ones that move funds or code first. */
const KINDS: ReadonlyArray<[ActionKind["tag"], string]> = [
  ["Transfer", "Transfers"],
  ["Upgrade", "Upgrades"],
  ["SetPolicy", "Policy changes"],
  ["AddMember", "New members"],
  ["RemoveMember", "Removing members"],
  ["ChangeRole", "Role changes"],
  ["InstallExtension", "New extensions"],
  ["UninstallExtension", "Removing extensions"],
]

/**
 * The approval policy: for each kind of action, how many approvals it needs
 * of the members who may approve, owners and approvers, and how long it waits
 * once it has them.
 */
export function Rules({
  wallet: id,
  members,
}: {
  wallet: Principal
  members: Member[] | undefined
}) {
  const { client, wallet } = useWallet(id)
  const policy = useQuery(client.queryOptions(wallet, "policy"))
  const approvers = members?.filter(
    (member) => member.role.tag === "Owner" || member.role.tag === "Approver",
  ).length
  const ruleOf = (kind: ActionKind["tag"]) =>
    policy.data?.rules.find((rule) => rule.kind.tag === kind)
  const transfer = ruleOf("Transfer")
  const setPolicy = ruleOf("SetPolicy")
  return (
    <section className="card">
      <div className="card-head">
        <ShieldIcon />
        <h2>Approval policy</h2>
      </div>
      {policy.isError ? (
        <p className="error">{failureText(policy.error)}</p>
      ) : policy.data === undefined ? (
        <p className="skeleton wide" />
      ) : (
        <>
          <table className="rules">
            <thead>
              <tr>
                <th scope="col">Action</th>
                <th scope="col">Approvals</th>
                <th scope="col">Wait</th>
              </tr>
            </thead>
            <tbody>
              {KINDS.map(([kind, name]) => {
                const rule = ruleOf(kind)
                return (
                  rule && (
                    <tr key={kind}>
                      <th scope="row">{name}</th>
                      <td>
                        {rule.quorum}
                        {approvers === undefined ? "" : ` of ${approvers}`}
                      </td>
                      <td>{rule.timelock_ns === 0n ? "None" : durationText(rule.timelock_ns)}</td>
                    </tr>
                  )
                )
              })}
            </tbody>
          </table>
          {transfer !== undefined && transfer.daily_limits.length > 0 && (
            <div className="limits">
              <p className="quiet small">
                Within a day&apos;s limit, an owner&apos;s transfer needs no other approval and no
                wait; a proposer&apos;s always waits for one:
              </p>
              <p className="chips">
                {transfer.daily_limits.map((limit) => (
                  <span key={chainAmountText(limit.amount, limit.chain)} className="chip">
                    {chainAmountText(limit.amount, limit.chain)} a day
                  </span>
                ))}
              </p>
            </div>
          )}
          {transfer?.quorum === 1 && approvers !== undefined && approvers > 1 && (
            <p className="hint">
              One approval moves funds, though {approvers} members can approve. A policy change can
              ask for more
              {setPolicy && setPolicy.timelock_ns > 0n
                ? `, and waits ${durationText(setPolicy.timelock_ns)}`
                : ""}
              .
            </p>
          )}
        </>
      )}
    </section>
  )
}
