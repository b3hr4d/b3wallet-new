import type { Principal } from "@candid-core/schema"
import { useMutation, useQuery } from "@tanstack/react-query"
import { useState } from "react"
import type {
  Account,
  AccountKind,
  Action,
  Contact,
  ContactAddress,
  Member,
  Pending,
} from "../generated/wallet.ts"
import { useWallet } from "../ic/canister.ts"
import { contactChange, plainName, recipientText } from "../logic/contacts.ts"
import { failureText } from "../logic/describe.ts"
import { dateOf } from "../logic/time.ts"
import { kindText } from "../logic/tokens.ts"
import { AlertIcon, PeopleIcon } from "./icons.tsx"

/** The owners' contacts, read for a ticket; undefined until read. */
export function useContacts(id: Principal): Contact[] | undefined {
  const { client, wallet } = useWallet(id)
  return useQuery(client.queryOptions(wallet, "contacts")).data
}

/**
 * What a contact action changes, for a ticket: each kind's old and new
 * address side by side, in full, and a warning for an address that looks
 * like one the owners already pay (decision 0015). For a transfer, who it
 * pays as the owners know them.
 */
export function ContactLines({
  action,
  from,
  accounts,
  contacts,
}: {
  action: Action
  from: Account | undefined
  accounts: Account[] | undefined
  contacts: Contact[] | undefined
}) {
  const recipient = recipientText(action, from, accounts, contacts)
  if (recipient !== undefined) {
    const known = !recipient.startsWith("To an address")
    return <p className={known ? "quiet" : "notice"}>{recipient}</p>
  }
  if (contacts === undefined || accounts === undefined) return null
  const change = contactChange(action, contacts, accounts)
  if (change === undefined) return null
  return (
    <>
      {change.name && (
        <p className="quiet">
          {change.name.before === undefined
            ? `A new contact, “${change.name.after}”.`
            : `Its name changes from “${change.name.before}” to “${change.name.after}”.`}
        </p>
      )}
      <table className="rules">
        <thead>
          <tr>
            <th scope="col">Address</th>
            <th scope="col">Now</th>
            <th scope="col">{action.tag === "RemoveContact" ? "After" : "Once it runs"}</th>
          </tr>
        </thead>
        <tbody>
          {change.addresses.map(({ kind, before, after }) => (
            <tr key={kindText(kind)}>
              <th scope="row">{kindText(kind)}</th>
              <td>{before === undefined ? "None" : <code>{before}</code>}</td>
              <td>
                {after === undefined ? "None" : <code>{after}</code>}
                {before !== undefined && after !== undefined && before !== after && " (changed)"}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {change.warnings.map((warning) => (
        <p key={warning} className="notice">
          <AlertIcon /> {warning}
        </p>
      ))}
    </>
  )
}

const KINDS: ReadonlyArray<AccountKind["tag"]> = ["InternetComputer", "Bitcoin", "Evm"]

/**
 * The owners' contacts on the settings page: their names and addresses, and
 * for an owner, a form to add or change one and a button to remove one. Each
 * is an action: adding or changing waits out the `SetContact` rule, a day by
 * default, before a transfer can pay it by name.
 */
export function Contacts({ wallet: id, me }: { wallet: Principal; me: Member }) {
  const { client, wallet } = useWallet(id)
  const contacts = useQuery(client.queryOptions(wallet, "contacts"))
  const status = useQuery(client.queryOptions(wallet, "status"))
  const propose = useMutation(
    client.mutationOptions(wallet, "propose", { invalidates: [[wallet, "pending"]] }),
  )
  const [editing, setEditing] = useState<number | null>(null)
  const [name, setName] = useState("")
  const [addresses, setAddresses] = useState<Record<string, string>>({})
  const [proposed, setProposed] = useState<Pending>()
  const owner = me.role.tag === "Owner"
  const network = status.data?.bitcoin_network
  const kindOf = (tag: AccountKind["tag"]): AccountKind | undefined =>
    tag === "Bitcoin" ? network && { tag: "Bitcoin", value: network } : ({ tag } as AccountKind)
  const edit = (contact: Contact) => {
    setEditing(contact.id)
    setName(contact.name)
    setAddresses(
      Object.fromEntries(contact.addresses.map((address) => [address.kind.tag, address.address])),
    )
  }
  const reset = () => {
    setEditing(null)
    setName("")
    setAddresses({})
  }
  const send = (action: Action) =>
    propose.mutate(action, {
      onSuccess: (result) => {
        setProposed(result)
        reset()
      },
    })
  const filled: ContactAddress[] = KINDS.flatMap((tag) => {
    const kind = kindOf(tag)
    const address = addresses[tag]?.trim() ?? ""
    return kind && address !== "" ? [{ kind, address }] : []
  })
  const refusal =
    name.trim() === ""
      ? "Give the contact a name."
      : !plainName(name.trim())
        ? "Write the name in plain English letters: A to Z, digits, spaces and punctuation."
        : filled.length === 0
          ? "Give it at least one address."
          : undefined
  return (
    <section id="contacts" className="card">
      <div className="card-head">
        <PeopleIcon />
        <h2>Contacts</h2>
        {contacts.data !== undefined && <span className="count">{contacts.data.length}</span>}
      </div>
      <p className="quiet small">
        A name and its addresses, one for each kind of account. Your agent can pay a contact by
        name, and the guard then checks the recipient against your words. A new contact or a changed
        address waits a day before it counts.
      </p>
      {contacts.isError ? (
        <p className="error">{failureText(contacts.error)}</p>
      ) : contacts.data === undefined ? (
        <span className="skeleton wide" />
      ) : contacts.data.length === 0 ? (
        <p className="quiet">No contacts yet.</p>
      ) : (
        <dl className="settings-rows">
          {contacts.data.map((contact) => (
            <div key={contact.id}>
              <dt>{contact.name}</dt>
              <dd>
                {contact.addresses.map((address) => (
                  <p key={kindText(address.kind)}>
                    <span className="quiet small">{kindText(address.kind)} </span>
                    <code>{address.address}</code>
                  </p>
                ))}
                {owner && (
                  <div className="buttons">
                    <button type="button" className="small" onClick={() => edit(contact)}>
                      Change
                    </button>
                    <button
                      type="button"
                      className="danger small"
                      disabled={propose.isPending}
                      onClick={() => send({ tag: "RemoveContact", value: contact.id })}
                    >
                      Propose removing it
                    </button>
                  </div>
                )}
              </dd>
            </div>
          ))}
        </dl>
      )}
      {proposed && (
        <p className="hint">
          Proposed as action {String(proposed.id)}. Approve and run it on the{" "}
          <a href="/approvals">Approvals page</a>
          {proposed.executable_after_ns > proposed.proposed_at_ns
            ? `, from ${dateOf(proposed.executable_after_ns)}`
            : ""}
          .
        </p>
      )}
      {owner && (
        <form
          className="form"
          onSubmit={(event) => {
            event.preventDefault()
            if (refusal !== undefined) return
            send({
              tag: "SetContact",
              value: { id: editing, name: name.trim(), addresses: filled },
            })
          }}
        >
          <h3>{editing === null ? "Add a contact" : `Change contact ${editing}`}</h3>
          <label>
            Name
            <input
              value={name}
              onChange={(event) => setName(event.target.value)}
              autoComplete="off"
            />
          </label>
          {KINDS.map((tag) => {
            const kind = kindOf(tag)
            return (
              kind && (
                <label key={tag}>
                  {kindText(kind)} address
                  <input
                    value={addresses[tag] ?? ""}
                    onChange={(event) => setAddresses({ ...addresses, [tag]: event.target.value })}
                    autoComplete="off"
                    spellCheck={false}
                  />
                </label>
              )
            )
          })}
          <p className="quiet small">
            Copy each address from the person, and check every character: a look-alike address is
            how funds are stolen. The Approvals page shows the old and the new address side by side.
          </p>
          <div className="buttons">
            <button
              type="submit"
              className="primary"
              disabled={refusal !== undefined || propose.isPending}
            >
              {propose.isPending ? "Proposing…" : "Propose it"}
            </button>
            {editing !== null && (
              <button type="button" onClick={reset}>
                Cancel
              </button>
            )}
          </div>
          {name.trim() !== "" && refusal !== undefined && <p className="quiet small">{refusal}</p>}
          {propose.isError && <p className="error small">{failureText(propose.error)}</p>}
        </form>
      )}
    </section>
  )
}
