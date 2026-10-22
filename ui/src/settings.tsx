import type { Principal } from "@candid-core/schema"
import { formatUnits } from "@ic-reactor/core"
import { useAuth } from "@ic-reactor/react"
import { useQuery } from "@tanstack/react-query"
import { useWallet } from "./canister.ts"
import { checkupOf } from "./checkup.ts"
import { CopyButton } from "./copy.tsx"
import { failureText, shortText } from "./describe.ts"
import { readFacts } from "./facts.ts"
import type { Member } from "./generated/wallet.ts"
import type { Check } from "./health.ts"
import { KeyIcon, LayersIcon, PeopleIcon, ShieldIcon } from "./icons.tsx"
import { initialOf } from "./mark.ts"
import { AsMember } from "./member.tsx"
import { useNowNs } from "./now.ts"
import { byRole, CHIP, Rules } from "./people.tsx"
import { ReleaseCheck } from "./release.tsx"
import { useRun } from "./sign-in.tsx"
import { leftText } from "./time.ts"
import { LOW_CYCLES } from "./tokens.ts"

/**
 * The settings page: how the wallet is set up and what protects it, in one
 * place. Its members, a security checkup with the approval policy, what the
 * IC says the wallet runs and who controls it, and your session.
 */
export function Settings({ wallet, local }: { wallet: Principal; local: boolean }) {
  return (
    <section className="column">
      <p className="back">
        <a href="/">Overview</a>
      </p>
      <div className="page-head">
        <h1>Settings</h1>
        <p className="quiet">How this wallet is set up, and what protects it.</p>
      </div>
      <nav className="section-nav" aria-label="Settings">
        <a href="#members">Members</a>
        <a href="#security">Security</a>
        <a href="#wallet">Wallet</a>
        <a href="#session">Session</a>
      </nav>
      <AsMember wallet={wallet}>
        {(me) => <Sections wallet={wallet} me={me} local={local} />}
      </AsMember>
    </section>
  )
}

function Sections({ wallet: id, me, local }: { wallet: Principal; me: Member; local: boolean }) {
  const { client, wallet } = useWallet(id)
  const members = useQuery(client.queryOptions(wallet, "members"))
  const policy = useQuery(client.queryOptions(wallet, "policy"))
  const status = useQuery(client.queryOptions(wallet, "status"))
  // Public state, read once a visit: it changes only with an upgrade.
  const facts = useQuery({
    queryKey: ["b3wallet", "facts", id, local],
    queryFn: () => readFacts(id, local),
    staleTime: Infinity,
    retry: false,
  })
  const checks = checkupOf({
    wallet: id,
    members: members.data,
    policy: policy.data,
    controllers: facts.data?.controllers,
  })
  return (
    <div className="stack">
      <section id="members" className="card">
        <div className="card-head">
          <PeopleIcon />
          <h2>Members</h2>
          {members.data !== undefined && <span className="count">{members.data.length}</span>}
          <div className="card-end">
            <a className="button small" href="/members">
              {me.role.tag === "Owner" ? "Manage members" : "See members"}
            </a>
          </div>
        </div>
        {members.isError ? (
          <p className="error">{failureText(members.error)}</p>
        ) : members.data === undefined ? (
          <span className="skeleton wide" />
        ) : (
          <div className="member-summary">
            <p className="avatars" aria-hidden="true">
              {[...members.data]
                .sort(byRole)
                .slice(0, 6)
                .map((member) => (
                  <span key={member.id} className="avatar">
                    {initialOf(member.label)}
                  </span>
                ))}
            </p>
            <p className="chips">
              {(["Owner", "Approver", "Proposer", "Viewer"] as const).map((role) => {
                const count = members.data.filter((member) => member.role.tag === role).length
                return (
                  count > 0 && (
                    <span key={role} className={CHIP[role]}>
                      {count} {role.toLowerCase()}
                      {count === 1 ? "" : "s"}
                    </span>
                  )
                )
              })}
            </p>
          </div>
        )}
      </section>

      <section id="security" className="card">
        <div className="card-head">
          <ShieldIcon />
          <h2>Security checkup</h2>
        </div>
        {checks.length === 0 ? (
          <span className="skeleton wide" />
        ) : (
          <ul className="health-list">
            {checks.map((check) => (
              <CheckItem key={check.title} check={check} />
            ))}
          </ul>
        )}
        {facts.isError && (
          <p className="quiet small">
            The IC&apos;s record of who controls the wallet could not be read:{" "}
            {failureText(facts.error)}
          </p>
        )}
      </section>
      <Rules wallet={id} members={members.data} />

      <section id="wallet" className="card">
        <div className="card-head">
          <LayersIcon />
          <h2>Wallet</h2>
        </div>
        <dl className="settings-rows">
          <div>
            <dt>Canister id</dt>
            <dd className="with-copy">
              <code>{id}</code>
              <CopyButton text={id} label="Copy the wallet's id" />
            </dd>
          </div>
          <div>
            <dt>Version</dt>
            <dd>
              <p>{status.data?.version ?? "…"}</p>
              <ReleaseCheck wallet={id} me={me} />
            </dd>
          </div>
          <div>
            <dt>Module hash</dt>
            <dd className="with-copy">
              {facts.data?.moduleHash ? (
                <>
                  <code title={facts.data.moduleHash}>
                    {shortText(facts.data.moduleHash, 12, 12)}
                  </code>
                  <CopyButton text={facts.data.moduleHash} label="Copy the module hash" />
                </>
              ) : facts.isError ? (
                <span className="quiet">Not read</span>
              ) : (
                <span className="skeleton" />
              )}
            </dd>
          </div>
          <div>
            <dt>Controllers</dt>
            <dd>
              {facts.data === undefined ? (
                facts.isError ? (
                  <span className="quiet">Not read</span>
                ) : (
                  <span className="skeleton" />
                )
              ) : (
                facts.data.controllers.map((controller) => (
                  <code key={controller} className="principal">
                    {controller === id ? "This wallet itself" : controller}
                  </code>
                ))
              )}
            </dd>
          </div>
          <div>
            <dt>Release channel</dt>
            <dd className="with-copy">
              {status.data ? (
                <>
                  <code>{status.data.release_channel}</code>
                  <CopyButton
                    text={status.data.release_channel}
                    label="Copy the release channel's id"
                  />
                </>
              ) : (
                <span className="skeleton" />
              )}
            </dd>
          </div>
          <div>
            <dt>Cycles</dt>
            <dd>
              {status.data ? (
                <>
                  <p className={status.data.cycles < LOW_CYCLES ? "error" : undefined}>
                    {formatUnits(status.data.cycles, 12, { maxFractionDigits: 2 })}T
                    {status.data.cycles < LOW_CYCLES ? ", low" : ""}
                  </p>
                  <p className="quiet small">Anyone can top it up:</p>
                  <p className="command">
                    <code>icp canister top-up --amount 1t {id}</code>
                    <CopyButton
                      text={`icp canister top-up --amount 1t ${id}`}
                      label="Copy the command"
                    />
                  </p>
                </>
              ) : (
                <span className="skeleton" />
              )}
            </dd>
          </div>
        </dl>
        <p className="quiet small">
          The module hash and controllers come from the IC&apos;s certified state. To check them
          without this page, run <code>icp canister status {id}</code>.
        </p>
      </section>

      <Session />
    </div>
  )
}

function CheckItem({ check }: { check: Check }) {
  return (
    <li className="health-item">
      <span className={`health-mark ${check.level}`} aria-hidden="true" />
      <div>
        <p className="health-title">
          {check.href ? <a href={check.href}>{check.title}</a> : check.title}
          {check.level !== "ok" && (
            <span className="visually-hidden">
              {check.level === "problem" ? " (a problem)" : " (needs attention)"}
            </span>
          )}
        </p>
        <p className="health-text">{check.text}</p>
      </div>
    </li>
  )
}

/** Who is signed in on this device, until when, and the sign-out. */
function Session() {
  const auth = useAuth()
  const { run, failure } = useRun()
  const nowNs = useNowNs()
  return (
    <section id="session" className="card">
      <div className="card-head">
        <KeyIcon />
        <h2>Session</h2>
      </div>
      <p className="field-label">Signed in as</p>
      <p className="address-box">
        <code>{auth.principal}</code>
        <CopyButton text={auth.principal} label="Copy your principal" />
      </p>
      <p className="quiet small">
        Internet Identity gives this wallet&apos;s site its own principal for you
        {auth.expiresAt === undefined
          ? "."
          : `; this session has ${leftText(BigInt(auth.expiresAt) * 1_000_000n, nowNs)}.`}
      </p>
      <div className="buttons">
        <button type="button" onClick={() => run(auth.signOut)}>
          Sign out
        </button>
      </div>
      {failure && <p className="error">{failure}</p>}
    </section>
  )
}
