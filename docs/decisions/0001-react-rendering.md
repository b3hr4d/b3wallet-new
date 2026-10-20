# 0001: How the wallet renders React

Status: proposed, 2026-10-20.

**Recommendation: prerender the React pages at build time, embed them and the
browser bundle in the signed release, serve them certified, and render every
private value in the browser from verified canister calls. No JavaScript
engine runs inside the wallet canister.**

Rendering React inside the canister works (Boa, measured below), but for a
wallet it adds code to the canister that holds the funds without making
anything the user sees more trustworthy. The two facts behind that:

1. **Certified HTML can only hold what is the same for every visitor.** HTTP
   certification certifies one response per path. A wallet's pages are almost
   all private (balances, history, members, pending approvals), so even a
   canister that renders HTML cannot put those values in certified HTML. They
   have to arrive through authenticated calls in every option.
2. **Calls are verified in the browser; HTML is verified by the gateway.** The
   agent in the page checks each update response's certificate against the IC
   root key, and query responses against node signatures, inside the browser.
   Certified HTML and JavaScript are checked by the HTTP gateway, so the
   browser trusts the gateway for the page code in every option, including
   in-canister rendering. Rendering in the canister does not change who
   verifies the page.

So the most secure split is: page code fixed at build time and covered by the
release signatures, private data fetched by the agent and verified where the
user is.

## Options compared

Measured numbers come from the local-network spikes of 2026-10-19 and
2026-10-20 (icp-cli 1.2.0, ic-cdk 0.20.3, React 19.2.0, esbuild 0.25.10). The
50-note page is the spike's test page, about 11.7 KB of HTML.

| Option | JavaScript engine in the wallet canister | Who verifies the page | Private data | Added to the wallet Wasm | Runtime cost | Main risk |
| --- | --- | --- | --- | --- | --- | --- |
| **A. Prerender at build, hydrate in the browser** | None | Gateway, against a certificate made from the release's own files | Agent calls, verified in the browser | Browser bundle 195 KB (61 KB gzipped) plus the HTML | 3.6M to 4.1M instructions to serve a certified file | The npm build's supply chain, which every option shares |
| B. Browser rendering only, from certified files | None | Same as A | Same as A | Same as A, minus the prerendered HTML | Same as A | Same as A; a blank page until the bundle runs |
| C. Render on write inside the wallet, Boa | Boa 0.22 (Rust; 95.60% of Test262) | Gateway, against a certificate made at write time | Cannot be certified, so still agent calls | Engine and server bundle: Wasm 4.4 MB (2.2 MB gzipped); browser bundle still needed to hydrate | 640M instructions to load React at install and every upgrade; 408M to render and certify the 50-note page on each write | An interpreter and a second bundle in the canister that holds the funds; forces chunked upgrades |
| D. Render on write inside the wallet, QuickJS | QuickJS, written in C, sharing the canister's linear memory | Same as C | Same as C | See below | See below | Memory-safety bugs in C next to wallet state; QuickJS-family engines had several in 2026 (for example CVE-2026-0821, a heap overflow in quickjs-ng) |
| E. Render on write in a separate canister on its own origin | None | Gateway, against that canister's certificate | Never there | None | Paid by that canister | Safe only for public pages, and only if it never serves the wallet's own origin |
| F. Whole canister in JavaScript (Azle) | QuickJS runs all canister logic | Same as C | Same as C | Azle's runtime | Not measured | Azle's README: stable mode "has not yet undergone intense security review" |
| G. Rust UI framework rendering in the canister (Leptos, Dioxus) | None; the renderer is Rust | Same as C | Same as C | Not measured | Not measured | Not React; no maintained IC integration found |
| H. Render on a server outside the IC | None | The server | The server sees it | None | The server's | A server every user must trust, outside the one-canister model |

QuickJS (option D) is being measured on the same page and bundle; this record
is updated with its numbers before the decision.

### Why not render inside the wallet (C, D, F, G)

- **More code where the funds are.** Every line in the wallet canister runs
  with the authority to propose, sign and upgrade. A JavaScript engine is the
  largest component the wallet would carry, larger than all of its own logic.
- **No integrity gain.** The page is still checked by the gateway, and the
  private values still come from calls (the two facts above).
- **Upgrade cost and risk.** Boa makes the Wasm 4.4 MB, so every upgrade must
  be chunked, and React is reloaded in every `post_upgrade` (640M
  instructions). A trap there leaves the old version running, but it is one
  more way for a release to fail its upgrade.
- **Cycles.** Re-rendering on every write costs about 408M instructions for the
  50-note page, against about 4M to serve a prerendered file.

### When rendering on the IC is still right

Public pages built from on-chain data, the same for every visitor (a public
payment request, a receipt), are where render on write shines. They belong in
option E: a separate canister with no funds and its own origin, so a bug in its
engine or its HTML can never reach the wallet's origin. Boa is the engine to
use there, because it is memory-safe Rust and it is the one measured.

## Rules that come with option A

- **Pages ship inside the release.** The prerendered HTML and the browser
  bundle are built reproducibly (pinned Node, esbuild and lockfile) and
  embedded in the wallet Wasm with `include_bytes!`. The release's Wasm hash,
  and so the release signatures, then cover every byte of page code. Release
  signers rebuild and compare before signing.
- **Served certified.** `ic-asset-certification` 4.0.0 certifies the embedded
  files in `init` and `post_upgrade`, with a certified 404.
- **No per-user data in HTML.** Prerendered pages are shells; every private
  value is rendered by React in the browser from an agent call.
- **Strict CSP, no inline script.** `script-src 'self'`, `frame-src 'none'`,
  `frame-ancestors 'none'`, `object-src 'none'`, `base-uri 'none'`, and a
  `connect-src` limited to the IC API hosts the agent uses; the exact header is
  tested in step 7.
- **React's escaping stays on.** A lint rule bans `dangerouslySetInnerHTML`.
  React 19.2.0 already blocks `javascript:` URLs, in its client and server
  renderers (checked in `react-dom` 19.2.0's production builds).

What this does not fix: the browser still trusts the HTTP gateway to verify the
page code. That holds for every option above. Users who need more can verify
locally, for example by running their own HTTP gateway.

## What changes if this is accepted

- `docs/design.md`: the HTTP and pages module serves embedded, certified files
  instead of rendering; the Pages store and the heap's JavaScript engine go
  away; render on write moves to the framework (v3) as option E.
- `docs/plan-v0.md`: step 7 serves the embedded files and tests the CSP;
  step 13 builds the prerendered pages and the browser app.

## Sources

- Boa and render-on-write measurements: the React-in-a-canister spike of
  2026-10-19 (React 19.2.0 on Boa 0.22.0, ic-http-certification 4.0.0).
- [Boa v0.22 release notes](https://github.com/boa-dev/boa/releases/tag/v0.22):
  95.60% of Test262.
- [CVE-2026-0821](https://nvd.nist.gov/vuln/detail/CVE-2026-0821): heap overflow in
  quickjs-ng's typed array constructor.
- [Azle README](https://github.com/demergent-labs/azle): the security-review
  disclaimer; its stable canister template depends on `rquickjs`.
- [ic-asset-certification](https://docs.internetcomputer.org/guides/frontends/certification/):
  certified static files for Rust canisters.
