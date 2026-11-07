# 0018: Passkeys approve on the wallet's own page, and a frontend canister holds the rest

Status: proposed, 2026-11-07. This record first proposed a thin approve
page signed in with Internet Identity. The direction was then chosen the
same day: "a separate frontend canister with a special asset canister for
each wallet which can also have disaster recovery to the wallet canister
and vise versa, the backend passkey only approve only from the
transaction which will serve directly from the wallet backend canister".
It builds on [decision 0017](0017-approve-only-ui.md) (pull request #45),
which made the start page the one page where members approve.

**Proposal: the wallet canister serves one page, where its members
approve, and nothing else. They sign there with passkeys of the wallet's
own origin. The IC checks the passkey's own signature on every call, so
each approval asks the person's authenticator, and there is no session key
to steal. The page shows the wallet's own words for each action, and
approves or rejects it. Each wallet also gets a frontend canister: the
IC's certified-assets canister, on an origin of its own and controlled by
the wallet, which serves the full UI. Whoever uses it is a member that
reads and proposes, and never approves. Owners may let anyone upload
another UI to it. Recovery runs both ways without a loop: the wallet
repairs its frontend, and the recovery controller of
[decision 0009](0009-recovery-controller.md), held by the owners'
recovery keys, repairs the wallet. Neither canister controls the other's
rescuer.**

## What the spike proved

On 2026-11-07 the real wallet Wasm, built from main at `f2b500e`, ran on
PocketIC 16.1.0 (the server CI pins) in live mode. Chromium 153 opened a
page at the wallet's own origin, `http://<wallet id>.localhost:<port>`,
with a virtual authenticator (CTAP2, platform, user verification on). The
page signed the owner's calls with `WebAuthnIdentity` from
`@icp-sdk/core` 6.1.1, the SDK the UI pins, and counted each time it
asked the authenticator.

| Step | What happened | Prompts |
| --- | --- | --- |
| Make a passkey, and the page's IC client with it | The passkey's principal is its own self-authenticating principal; the client reads the root key unsigned | 0, besides registering the passkey |
| `claim`, signed by the passkey, with the claim link's signature | The passkey is the wallet's first owner | 1 |
| `whoami`, a query | Owner | 2 the first time, then 1 |
| `request_join` from an agent's own key | A join request waits for one approval | 0 |
| `pending`, as an update call | The owner reads the join request | 1 |
| `approve` | Ready | 1 |
| `execute` | Executed; the agent is a proposer | 1 |
| The same passkey, with one bit of its ECDSA signature flipped | The replica refuses it: "Invalid signature: WebAuthn error: Verifying signature failed" | 1 |
| Another origin names the wallet's RP ID | The browser refuses: "SecurityError: The relying party ID is not a registrable domain suffix of, nor equal to the current domain. Subsequently, an attempt to fetch the .well-known/webauthn resource of the claimed RP ID failed." | - |
| Another origin asks for the passkey for itself | The browser finds none: "NotAllowedError" | - |

So every update call asks the authenticator exactly once. The extra
prompt of the first query is the IC client's read of the subnet's node
keys, which it needs to verify a query's reply. It signs that read with
the same identity, and keeps the keys (`fetchSubnetKeys` in
`@icp-sdk/core`). The IC itself checks the signature, so only that
passkey acts as that member, and the passkey works only on the wallet's
origin. Approving one action from a link takes two prompts: one to read
its words, one to approve. Not tried: mainnet, a real platform
authenticator, Safari and Firefox.

## What bounds the design

1. **Only the wallet's own origin approves.** A passkey belongs to one
   origin (its RP ID), and only the canister serves
   `https://<wallet id>.icp0.io`. Whatever approves runs there, from
   files in the signed release.
2. **A canister cannot put private values in HTML.** An HTTP request
   carries no caller, and certification covers one response per path for
   every visitor ([decision 0001](0001-react-rendering.md)). TLS ends at
   the HTTP gateway, so a session cookie or token would be a bearer secret
   the gateway reads in the clear. Private values come only from calls the
   browser signs and verifies.
3. **Signing and verifying need the IC's SDK.** In pull request #45's
   `app.js` (559,339 bytes), `@icp-sdk/*`, `@noble/*`, `@dfinity/cbor`
   and `idb` are 252,682 bytes. A page that signs with a passkey and needs
   no Internet Identity client measured 206,191 bytes (67,510 with
   `gzip -9`), its own code included.

## The three canisters

| Canister | Serves | Its controllers | What it holds | What it can do to the wallet |
| --- | --- | --- | --- | --- |
| Wallet | The approve page, at `/`, `/approvals` and `/claim`, and its API | Itself; or the recovery controller, once owners hand over its code (0009) | Funds, policy, members and their passkeys, and the release's UI for the frontend | - |
| Frontend | The full UI, at `https://<frontend id>.icp0.io` | The wallet, and nothing else | UI files | What its users' member principals may do: read and propose |
| Recovery controller | Nothing | None | The owners' recovery keys and their operations | Install a fix, after the keys' threshold and delay |

## The approve page

1. **Passkeys, not Internet Identity.** An Internet Identity session is a
   key in the browser that signs anything until it expires. A passkey asks
   the person's authenticator for each call, and a look-alike site cannot
   use it. The IC checks WebAuthn signatures itself, so the wallet adds no
   cryptography.
2. **Few calls.** Each call asks the authenticator, so the page makes one
   read and one call per decision. The read is a new members-only method,
   `approvals`. It returns each open action with the wallet's words for
   it, and the buttons the caller's role allows. The words are
   `ui/src/logic/describe.ts`'s, ported to Rust. ICRC-21, which
   `wallet.did` already declares, gives other clients the same words, one
   call per action.
3. **From the transaction.** The agent or the frontend gives the owner a
   link to one action, `https://<wallet id>.icp0.io/#<action id>`. The
   fragment never leaves the browser. The page reads that action's words
   and asks once more, to approve or reject it.
4. **A member is a person, with several passkeys.** An approval counts
   once per member. Otherwise one person with two passkeys would be two
   approvals of a quorum. Adding a passkey to yourself is an action your
   existing passkey approves, with the join code shown on both screens, as
   `request_join` does today.
5. **The wallet keeps each passkey's public key.** WebAuthn returns no
   public key when it signs, and the IC client needs it to name the sender. A
   new device asks the authenticator once to learn which passkey it holds,
   then reads that passkey's key from the wallet by its credential id, and
   keeps it.
6. **Never `/.well-known/webauthn`.** Chrome looks for it (the error
   above). A list of origins there would lend the wallet's passkeys to
   them. The wallet's certified 404 covers the path today, and a test will
   pin it.
7. **Approving, not running.** `execute` can outlast a synchronous call:
   a Bitcoin transfer signs each input with threshold ECDSA. The page's IC
   client then polls with signed reads, and each one would ask the passkey
   again.
   So the page approves and rejects, and a member that does not sign with
   a passkey runs the action, such as the frontend's or the agent's.
8. **The claim makes the first passkey.** The claim page registers the
   first owner's passkey and claims the wallet with it, as the spike did.

What stays from the first version of this record: the page is three small
files of the wallet's own, written as they ship, with one pinned file of
the SDK. It is certified and served with today's headers and CSP. The
wallet, not the page, writes the words. The wallet also refuses a
transfer outside the terms an owner confirmed, which only the page checks
today ([decision 0014](0014-confirmed-terms.md)).

## The frontend canister

- **What it is:** the IC's certified-assets canister, which the IC's
  documentation calls "the recommended way to host a frontend on ICP".
  "Every response it serves is certified, and it accepts only version 2
  of the certification protocol, so it has no uncertified serving path."
  Files reach it by `icp deploy`'s sync.
- **Who controls it:** the factory creates it beside the wallet and leaves
  the wallet as its only controller. The wallet keeps it topped up with
  cycles, within a cap the owners set.
- **Who uses it:** its users sign in on its origin, and join the wallet
  as members that read and propose. Internet Identity is the easiest
  sign-in there, since today's `ui/` app already uses it, and nothing on
  that origin approves. Today's Proposer proposes only transfers. The
  frontend needs a role that proposes every kind of action and makes the
  calls that move nothing: accounts, tracked tokens, the owner's requests
  and the guard's file. Call it Operator. It never approves, rejects or
  confirms terms.
- **Hackable:** owners approve an action that lets a principal sync files
  to the frontend, and the wallet carries it out with the canister's
  `authorize`: "Controllers are always authorized without being listed,
  and are the only callers that can change the set." That principal then
  uploads any UI with icp-cli. Owners take the right back the same way,
  and the wallet's repair restores the release's UI.
- **A hostile UI** on the frontend reads what its member reads, and
  proposes. It cannot approve: no passkey of the wallet works on its
  origin. Whatever it proposes, the approve page shows in the wallet's
  words.

## Recovery both ways, without the loop

| When this breaks | This still works | This repairs it |
| --- | --- | --- |
| The frontend: a bad upload, a bug, no cycles | The approve page and the wallet's API | The wallet. On an action owners approve, it reinstalls the frontend and restores the release's UI. It tops up cycles itself |
| The wallet's approve page, passkey sign-in, API or upgrade method | The frontend, with whatever of the wallet's API still answers, and the recovery controller's operations, which anyone may read | The recovery controller, on operations the recovery keys signed offline with `recovery-tool` |
| Both | The recovery controller | The recovery controller; then the wallet repairs the frontend |

**Why the frontend holds no power over the wallet's code.** Decision 0009
read Orbit's source, where the wallet and its upgrader control each
other, and rejected it: "The wallet's code controls its rescuer: a
malicious station release can upgrade the upgrader or change its
controllers." A frontend that controls the wallet while the wallet
controls the frontend is that loop. And the frontend is the part owners
open to other people's UIs, so whoever reached its code would hold every
fund.

**If one canister must be both frontend and rescuer.** 0009's controller
has no controller. So the wallet could no longer repair the frontend, only
replace its files. Its asset code would also sit beside the right to
install the wallet, and could never be fixed in place. Not recommended.

## What it takes

One pull request per step.

1. **The wallet's words.** `approvals` and ICRC-21, the words ported from
   `describe.ts`. Tests pin the sentences `ui/test/describe.test.ts` pins,
   and amounts come from `meant-format`.
2. **The terms check in the wallet.** `approve` and `execute` refuse a
   transfer outside the confirmed terms. The wallet already stores those
   terms as canonical JSON (`token`, `to`, `amount.units`, `ends.at`).
3. **Passkey members.**
   - A member's passkeys, with their credential ids and public keys.
   - The claim with a passkey, and adding a passkey to yourself.
   - The 404 at `/.well-known/webauthn`, pinned by a test.

   The spike becomes this step's PocketIC and browser test.
4. **The approve page in the wallet:** `approve.html`, `approve.css`,
   `approve.js` and `ic.js`, at `/`, `/approvals` and `/claim`. Internet
   Identity leaves the wallet's origin. Existing members add a passkey in
   step 3, while Internet Identity still works.
5. **The Operator role,** and who runs an approved action.
6. **The frontend canister.**
   - The factory creates it.
   - The wallet controls it, repairs it and tops it up.
   - Today's `ui/` moves there, as the release's default UI.
7. **The recovery controller,** offered when a wallet is created. Decision
   0009 built it already.

Steps 1 to 4 make the wallet approve-only, by passkey. Steps 5 and 6 move
everything else out. Step 7 stands on its own.

## What it does not fix

- The browser trusts the HTTP gateway to verify each page, in every
  design ([decision 0001](0001-react-rendering.md)).
- The authenticator shows nothing of what it signs. The owner reads the
  wallet's words on the wallet's page, and a release that lies is what the
  release signers are for.
- A synced passkey follows the person's Apple or Google account, and
  whoever takes that account takes the passkey. Large wallets want a
  hardware security key, and a quorum of two with timelocks.
- A hostile UI on the frontend reads what its member reads.

## Decisions this needs

1. A member with several passkeys (recommended), or one passkey per
   member.
2. Who runs an approved action: the frontend's or the agent's member
   (recommended), the approve page as well (more prompts), or the wallet's
   own timer.
3. A new Operator role (recommended), or a wider Proposer.
4. The frontend:
   - the certified-assets canister (recommended);
   - the legacy asset canister, whose Prepare and Commit split would let
     owners approve each upload;
   - or a canister of our own.
5. Sign-in on the frontend: Internet Identity (recommended, the easiest),
   or a passkey of that origin with a session key.
6. The wallet's rescuer: the separate controller of 0009 (recommended), or
   the frontend.
7. The words written by hand in Rust (recommended), or generated from a
   Meant profile of `wallet.did`. And `meant-format` in the wallet.
8. "From the transaction" in two prompts (recommended first), or in one.
   One prompt needs a key in the link's fragment, which lets the page read
   that one action's words unsigned.

## How it was measured

- **Sizes** were measured with pull request #45's
  `ui/package-lock.json` and its pinned esbuild 0.28.2, minified as
  `ui/build.mjs` minifies `app.js`, with `gzip -9`. Bundling #45's
  `src/main.tsx` the same way gives 559,339 bytes, the size of the
  `app.js` #45 commits, so the bytes per package are that file's. The
  passkey page is the spike's page bundled alone.
- **The spike** had three parts:
  - an ignored PocketIC test that installed the wallet with a claim key,
    made the instance live and ran a Node driver;
  - the driver: Playwright's Chromium, with a virtual authenticator added
    over the DevTools protocol. It served the bundled page at the wallet's
    origin by routing two paths, and every `/api` request went to the
    PocketIC gateway;
  - the claim signature, made with the claim key's seed.

  The page type-checks with TypeScript 6.0.3, in strict mode, against
  `@icp-sdk/core` 6.1.1. It ran three times with the same counts.
- **Reading `pending` without knowing an action.** Values encoded with
  the wallet's generated bindings decode with a page type that leaves
  `action` out. With one state case dropped from that type, decoding
  fails: a record may leave fields out, but a variant may not leave out a
  case.

## Sources

- [The IC interface specification](https://docs.internetcomputer.org/references/ic-interface-spec),
  "Signatures": WebAuthn signatures (ECDSA on P-256, RSA PKCS#1 v1.5 and
  Ed25519), the DER-wrapped COSE public key, and self-authenticating
  principals.
- [Application canisters](https://docs.internetcomputer.org/references/application-canisters/):
  the certified-assets canister and the legacy asset canister, and their
  permissions.
- [The Internet Identity specification](https://docs.internetcomputer.org/references/ii-spec):
  a principal for each origin.
- [Wallet integration](https://docs.internetcomputer.org/guides/digital-assets/wallet-integration/):
  ICRC-21, -25, -27, -29 and -49.
- [Decision 0009](0009-recovery-controller.md): Orbit's station and
  upgrader, read at `dfinity/orbit@c766e61`.
