# 0018: A thin approve page, in the wallet's own words

Status: proposed, 2026-11-07. Asked for: a new way for the canister to
serve only the part of the wallet that approves, the easiest, the safest
and the most hackable. It builds on
[decision 0017](0017-approve-only-ui.md) (pull request #45), which made
the start page the one page where members approve.

**Proposal: the wallet serves its approve page as three small files of
its own, `approve.html`, `approve.css` and `approve.js`, written as they
ship, beside one pinned file of the IC's SDK, `ic.js`. The page is a thin
terminal. It signs a member in with Internet Identity, lists the open
actions, shows for each only the words the wallet writes for that call,
and calls `approve`, `reject` or `execute`. The wallet writes those words
in Rust, from the action it stores, through the ICRC-21 method
`wallet.did` already declares and the wallet does not implement yet. The
wallet also checks a transfer against the terms an owner confirmed, which
only the page checks today. The files are certified and served with the
same headers and Content-Security-Policy as every page today. No
framework, no state library and no prerender ship with them.**

Measured with the UI's pinned esbuild 0.28.2, minified, and `gzip -9`
("How it was measured" below):

| What a member downloads to approve | Bytes | `gzip -9` |
| --- | --- | --- |
| `app.js` on main, which every page loads | 592,882 | 188,515 |
| `app.js` of pull request #45 | 559,339 | 179,838 |
| This proposal: `ic.js` and `approve.js` | 252,896 + 7,982 | 81,292 + 2,780 |
| The same page, with a passkey in place of Internet Identity (option E) | 206,191 | 67,510 |
| A thin page that keeps today's wording and terms code (option C) | 353,736 | 113,447 |

The page's own code is about 8 KB of plain JavaScript. The rest is the
IC's SDK, which any page that signs calls and verifies their answers
carries.

## What bounds every option

1. **Only the wallet's own origin approves.** Internet Identity gives each
   origin its own principal, so a member's principal for this wallet
   exists only in a page at `https://<wallet id>.icp0.io`, and only the
   canister serves that origin. Whatever signs as a member runs there.
2. **A canister cannot put private values in HTML.** An HTTP request
   carries no caller, and certification covers one response per path for
   every visitor ([decision 0001](0001-react-rendering.md)). TLS ends at
   the HTTP gateway, so a session cookie or token would be a bearer secret
   the gateway reads in the clear, and could use to read or to approve.
   Private values come only from calls the browser signs and verifies.
3. **Signing and verifying need the IC's SDK.** The page signs each call
   with a session key or a passkey, and checks each certified answer
   against the IC's root key. In pull request #45's `app.js`,
   `@icp-sdk/*`, `@noble/*`, `@dfinity/cbor` and `idb` are 252,682 bytes.
   No option goes below that, and an agent of our own would not be safer.

So the question is not where the page is rendered. It is what the page
has to decide by itself, and today it decides a lot.

## What the approve page decides today

Pull request #45's start page is the React app on Preact, and its
`app.js` holds every page:

| In `app.js` (559,339 bytes) | Bytes | Share |
| --- | --- | --- |
| `@icp-sdk/core`, `/auth` and `/signer`, `@noble/*`, `@dfinity/cbor`, `idb`: signing and verifying | 252,682 | 45% |
| Preact, ic-reactor, TanStack Query | 107,645 | 19% |
| The app's own code, every page's | 95,683 | 17% |
| `@candid-core/schema` and the generated bindings | 62,753 | 11% |
| `@meantjs/*`: names, amounts, memos and terms | 29,799 | 5% |
| `uqr` | 10,289 | 2% |

The other 488 bytes are esbuild's own. Three things the page decides
matter for what an owner approves:

- **The words.** `ui/src/logic/describe.ts` decodes each action and
  writes what it does (`actionText`, `actionDetails`, `stateText`), in
  about 170 lines. The owner approves what those lines say, and the call
  carries only the action's id. A bug in them can misstate an action, and
  a hostile package in the bundle can show one action and approve
  another.
- **The terms check** ([decision 0014](0014-confirmed-terms.md)). The page
  reads the agent's draft with `@meantjs/consent/terms`, writes it in
  words, and offers no Approve and no Run for a transfer outside the
  confirmed terms. The wallet stores what the page confirmed and checks
  only that the record holds together (`canisters/wallet/src/terms.rs`).
  A member's other client, or a broken page, approves anyway.
- **What a review sees.** `.gitattributes` marks `app.js` `-diff`: a pull
  request shows the shipped code as a change of size, and CI checks that
  it is the build's output.

## Options

| Option | What it is | For | Against |
| --- | --- | --- | --- |
| A. Keep 0017's start page | Pull request #45 as it is | Done; holdings, accounts, requests and approvals on one page | 559,339 bytes to approve; the words and the terms check are page code; three frameworks in the signed release |
| **B. A thin page in the wallet's own words** | The files above, in `canisters/wallet/approve/`; ICRC-21 and the terms check written in Rust | The smallest page that can approve; the words come from the canister, from the stored action, in a certified reply; the wallet refuses what only the page refused; the shipped page code reads in review and in the browser; any client gets the same words | The wallet must write the words and check the terms in Rust; one call per open action for its words; the page shows approvals, not holdings |
| C. A thin page that keeps the words in JavaScript | B's page with `logic/`, the bindings and `@meantjs/*` | No canister change | 353,736 bytes; the words and the checks stay page code |
| D. Render the page in the canister for each request | Rust writes HTML per request, through `http_request_update` or a cookie session | One language | Fact 2: a request has no caller, and a cookie is a bearer secret the gateway reads. Each load is an update call, seconds long and paid in cycles |
| E. A passkey per approval | Calls signed by a passkey of the wallet's origin, which the IC takes as a WebAuthn signature (`WebAuthnIdentity`, in `@icp-sdk/core` 6.1.1), in place of an Internet Identity session | No session key in the browser to steal; every call asks the authenticator; 206,191 bytes | Each passkey is a new member principal; every call prompts, reads included; the authenticator shows nothing of what it signs, so it stops a stolen session, not a lying page |
| F. Approve in an external signer | A signer that speaks ICRC-25, -29 and -49 and shows ICRC-21 messages, such as OISY's | A second screen the wallet's page cannot touch | Not served by the wallet; the member is that signer's principal; possible only once B's words exist |
| G. A JavaScript engine in the canister, or a Rust UI framework compiled to Wasm | | | Rejected by [decision 0001](0001-react-rendering.md) and [decision 0011](0011-preact-compat.md) |

## Why B is the easiest, the safest and the most hackable

**Easiest.**

- The page only signs in, reads, shows text and offers three buttons. It
  has no hydration, cache, provider or prerender.
- Its files are static. `canisters/wallet/src/http.rs` certifies them as
  it certifies `app.js` today, with the same headers, so the CSP does not
  change.
- `approve.js` ships as it is written, and `tsc --checkJs` checks it.
  Only `ic.js` is built, and it changes only when the SDK's pinned version
  does.
- The wallet's interface does not change. `icrc10_supported_standards`
  and `icrc21_canister_call_consent_message` are in `wallet.did` already,
  among the five methods not implemented yet ("implemented 35 of 40", the
  interface test in `canisters/wallet/src/lib.rs`).

**Safest.**

- The words come from the code that runs the action, from the bytes it
  stores, in a certified reply. The page never decodes an action: its
  `Pending` type has no `action` field.
- Only the IC's SDK can act as the member. Preact, ic-reactor, TanStack
  Query, `@candid-core/schema` and `@meantjs/*` leave the page that
  approves.
- The terms check moves from a hidden button to a refusal by the wallet,
  for every client (step 2).
- A review reads the page's code in the diff.
- The page and the wallet ship in one release, so the page's partial
  types always match the wallet that serves it.

**Most hackable.**

- The words are a standard call. `icp canister call`, the MCP server, an
  ICRC-21 signer or another page get the text the owner reads.
- The page is one readable file, and view-source shows it as it was
  written. Changing it takes a text editor and `cargo build`.
- Everything else can be built anywhere. A dashboard on another origin,
  a command-line tool or an agent joins as a Viewer or a Proposer
  (`request_join`), then reads or proposes. The one thing it cannot do is
  approve, which happens only on the wallet's own page. That is decision
  0017's approve-only screen taken all the way.

## What it takes

One pull request per step.

1. **The wallet writes the words.** `icrc21_canister_call_consent_message`
   answers for `approve`, `reject` and `execute`, and for `propose` and
   `propose_for`, whose words the agent can show its user. It answers
   members only, refusing anyone else before decoding, as every member
   method does. The words are `describe.ts`'s, ported to Rust in a new
   `consent.rs`:
   - the account's label, and the amount in the token's symbol and
     decimals, which the wallet already keeps for each tracked ledger;
   - the recipient with its account or contact name, and the memo;
   - a policy's rules and an upgrade's hash;
   - a join request's code, who proposed the action and when, and where
     it stands.

   Tests pin the sentences `ui/test/describe.test.ts` pins, so no owner
   reads a different sentence. Amounts come from `meant-format`, Meant's
   Rust port of `@meantjs/format`. It has no dependencies, and is checked
   against 3,412 cases of the JavaScript. It is a new dependency to
   decide. `icrc10_supported_standards` names ICRC-10 and ICRC-21.
2. **The wallet checks the terms.** `approve` and `execute` refuse a
   transfer that answers a request but falls outside its confirmed terms:
   another token or recipient, more than the most, or after the end. The
   wallet reads these from the confirmed terms it already stores, the
   canonical JSON `@meantjs/consent/terms` wrote (`token`, `to`,
   `amount.units`, `ends.at`), with the `serde_json` the guard already
   links. This is the transfer half of the library's `checkGrant`, and
   its tests follow `ui/test/terms.test.ts`. Reading an agent's draft and
   writing it in words stay in the page.
3. **The page.** It lives in `canisters/wallet/approve/`:
   - `approve.html`, a shell with no inline script or style;
   - `approve.css` and `approve.js`;
   - `ic.ts`, built to `ic.js` by esbuild from a lockfile of its own. CI
     builds it twice and compares, as it does `ui/` today.

   `http.rs` serves the page at `/` and `/approvals`. It holds:
   - the open actions, with the wallet's words and the buttons the
     member's role allows;
   - a join request's code;
   - the guard's flags for a transfer. `guard_check` stays a query, as
     [decision 0012](0012-guard-in-wallet.md) requires, and the page says
     these are the guard's reading.

   The owner's requests and the box that confirms their terms stay on
   today's app, with settings, members and the claim, until they are
   rewritten or moved. Both use `@icp-sdk/auth`'s `AuthClient` on one
   origin, so one sign-in should serve both (to check). The checks of
   [ui-standard.md](../ui-standard.md) that apply to a page become a test
   on `approve.js`. It must not hold `eval(`, `new Function(`,
   `innerHTML`, `outerHTML`, `insertAdjacentHTML`, `document.write` or
   `srcdoc`. Its budget is 20,000 bytes, and `ic.js`'s is 260,000.
4. **Later, if wanted:**
   - reading the terms in the wallet as well, so that confirming them
     moves to the thin page. This ports the reading half of
     `@meantjs/consent/terms` to Rust. It belongs in Meant, beside the
     consent renderer already ported for canisters
     (`profile/packages/consent/rust/`, `meant-profile-consent`), with a
     differential corpus;
   - a passkey per approval (E);
   - approving from an external signer (F), which rests on step 1.

What does not change: the policy, the roles, Internet Identity sign-in,
the CSP, certification, and every other check the wallet makes on a call.

## What it does not fix

- The browser trusts the HTTP gateway to verify the page, in every option
  ([decision 0001](0001-react-rendering.md)).
- Neither Internet Identity nor a passkey shows what it signs. The owner
  reads the words on the wallet's page. A release that lies is what the
  release signers are for.
- The words cost one update call per open action. If that is slow, the
  wallet can answer for all of them in one call.

## Decisions this needs

1. The words written by hand in Rust now (recommended), or generated from
   a Meant profile of `wallet.did`. That is
   [decision 0013](0013-meant-format.md)'s option B. Its first gap, an
   intent and an effect for a variant case, is Meant's D-053 now, whose
   example is `propose(0).Transfer`; the other gaps are still open.
2. `meant-format` in the wallet.
3. The terms check in the wallet (step 2, recommended), with the terms
   box on today's app until the reading moves to Rust.
4. The holdings and accounts that pull request #45 put on the start page:
   on settings, or on the thin page.
5. Settings and the claim: on today's app for now (recommended), or thin
   pages next.

## How it was measured

On 2026-11-07, with pull request #45's `ui/package-lock.json`, installed
by `npm ci --ignore-scripts`:

- **The split of `app.js`.** Pull request #45's `src/main.tsx` was bundled
  with `ui/build.mjs`'s aliases and options, with a metafile. The bundle
  is 559,339 bytes, the size of the `app.js` pull request #45 commits, so
  the bytes per package are that file's.
- **The thin page.** A spike page signs in with `AuthClient`, reads
  `whoami` and `pending` as update calls, asks
  `icrc21_canister_call_consent_message` for the words of each open
  action's `approve`, and shows them with `textContent`. It makes every
  element with `createElement`, and offers Approve, Run and Reject. It was
  bundled as `ui/build.mjs` bundles `app.js` (ESM, ES2022, minified, for
  the browser). `ic.js` is one file exporting `AuthClient`, `Actor`,
  `HttpAgent`, `IDL`, `WebAuthnIdentity` and `Principal`. `approve.js` is
  the spike with its types stripped and its comments kept: 7,982 bytes in
  181 lines. The spike does not show the guard's flags or a join request's
  code yet; they would add to it.
- **Option E** is the same page, signed in by `WebAuthnIdentity` instead
  of `AuthClient`. **Option C** is the same page importing pull request
  #45's `logic/describe.ts`, `logic/terms.ts`, the bindings and the
  schema codec.
- **Checks.** The spike type-checks with TypeScript 6.0.3, in strict mode,
  against `@icp-sdk/core` 6.1.1 and `@icp-sdk/auth` 11.0.0. No bundle
  holds a string `ui/build.mjs` refuses.
- **The page reads `pending` without knowing an action.** A transfer and
  an upgrade were encoded with the wallet's generated bindings. The page's
  partial type decoded them and saw `id`, `state`, `approvals`, `quorum`
  and `executable_after_ns`. With `Executed` dropped from the page's state
  type, decoding fails. So a record may leave fields out, but a variant
  may not leave out a case.
- **Not run:** the page against a wallet. The wallet has no ICRC-21 yet,
  and Internet Identity does not sign in from where this was measured.

## Sources

- [The IC interface specification](https://docs.internetcomputer.org/references/ic-interface-spec),
  "Signatures": WebAuthn signatures (ECDSA on P-256, RSA PKCS#1 v1.5 and
  Ed25519), the DER-wrapped COSE public key, and self-authenticating
  principals.
- [The HTTP gateway protocol](https://docs.internetcomputer.org/references/http-gateway-protocol-spec):
  `http_request_update`.
- [The Internet Identity specification](https://docs.internetcomputer.org/references/ii-spec):
  a principal for each origin, and `derivationOrigin`.
- [Wallet integration](https://docs.internetcomputer.org/guides/digital-assets/wallet-integration/):
  the signer standards ICRC-21, -25, -27, -29 and -49.
- B3Pay/Meant: `crates/meant-format`, and `profile/packages/consent/rust/`.
