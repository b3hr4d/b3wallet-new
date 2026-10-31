# The UI standard

How `ui/` is laid out, and the rules that keep the wallet's pages safe and
fast. Every rule names the check that enforces it: lint, the build, the CSP
or a test. A rule with no check is a wish, so there are none here.

## Why the UI needs a standard

The wallet serves its own pages ([decision 0001](decisions/0001-react-rendering.md)),
so its UI runs at the wallet's origin, `https://<wallet id>.icp0.io`. That
is the only origin where a member has their Internet Identity principal for
this wallet. Sharing a canister with the backend means two different things:

- **The canister side is static.** `http_request` answers only as a query,
  with files embedded in the signed release and certified at install. It
  reads no wallet state and can change none. A page cannot reach the
  canister's memory: it calls the same Candid interface as any other client,
  and the canister checks every call (role, quorum, timelock, daily limit)
  whatever the page shows.
- **The risk is in the browser.** Code that runs on the wallet's page acts
  as the signed-in member for as long as their session lasts. Code gets
  there in one of two ways: injected into the page (cross-site scripting),
  or shipped in the release (a dependency). This standard closes the first
  and narrows the second.

What no UI rule can do: make a malicious release safe (the release signers
and the review of each change do that, [decision 0004](decisions/0004-release-signers.md)),
or stop a member approving what they did not read. The canister's policy
bounds what a page acting as one member can do: a transfer above the daily
limit waits for its quorum and timelock.

## Layout

```text
ui/src/
  main.tsx         the browser's entry: hydrates the prerendered page
  prerender.tsx    the build's entry: renders each page to HTML in Node
  app.tsx          the frame both render, the same for every visitor
  live.tsx         what only the browser renders: the client, the rail, the page's own part
  app.css
  pages/           one module per page
  parts/           what pages share: components and hooks
  ic/              the only code that reaches the IC
  logic/           plain TypeScript: no React, no browser, no network
  generated/       the Candid bindings, written by every build
```

| Layer | What it holds | It may import | It may not |
| --- | --- | --- | --- |
| Entry points | `main.tsx`, `prerender.tsx`, `app.tsx`, `live.tsx` | Everything | Preact by name; `react-dom` outside `main.tsx`, `prerender.tsx` and `live.tsx` |
| `pages/` | Home, send, receive, account, members, inbox, settings, approvals, claim | `parts/`, `ic/`, `logic/`, `generated/` | Another page, an entry point |
| `parts/` | The frame's pieces, the rail, shared hooks such as `useDecide` | Other parts, `ic/`, `logic/`, `generated/` | A page, an entry point |
| `ic/` | `client.ts` (the client and sign-in), `canister.ts` (the wallet), `ledger.ts` (ledger metadata), `facts.ts` (the IC's certified state) | `logic/`, `generated/` | A part, a page, an entry point |
| `logic/` | What the pages say and decide: descriptions, health, tokens, time, QR codes | `generated/` | React, a client, the browser's globals |

Lint (`ui/eslint.config.js`) enforces every column, and `test/lint.test.ts`
checks the rules still refuse what they should.

## Security

1. **Only the release's own script runs.** The CSP allows script only from
   the wallet's origin, with no `'unsafe-inline'` and no `'unsafe-eval'`. The
   wallet's tests pin the CSP, lint refuses `eval`, `new Function` and string
   timers in the app, and the build refuses a bundle whose code holds `eval(`
   or `new Function(`, dependencies included.
2. **No markup from strings.** Pages render text and elements. Lint refuses
   `dangerouslySetInnerHTML`, `innerHTML`, `outerHTML`, `insertAdjacentHTML`
   and `document.write` in the app, and the build refuses the same in the
   bundle. The CSP requires Trusted Types and lets no code create a policy
   (`require-trusted-types-for 'script'; trusted-types 'none'`), so a
   browser that enforces them refuses, at run time, any string written to a
   markup or script sink, whatever code writes it.
3. **Links stay on the wallet's origin.** React 19 refuses a `javascript:`
   URL in a link; Preact, which the UI ships on ([decision 0011](decisions/0011-preact-compat.md)),
   does not. Lint refuses a `href`, `src`, `action` or `formAction` that is
   not a path from `/` or a `#fragment`, and any `javascript:` string; the
   tables of links type theirs as `Path` (`logic/page.ts`), so TypeScript
   refuses anything else. In the browser, the CSP blocks `javascript:` URLs
   twice: as inline script, and through Trusted Types.
4. **Only `ic/` reaches the network.** Lint refuses `fetch`,
   `XMLHttpRequest`, `WebSocket`, `EventSource` and `navigator.sendBeacon`
   outside `ic/`, and anything that makes a client, agent or actor or signs
   in (`createClient`, `useClient`, `HttpAgent`, `Actor`, `AuthClient`,
   `ReactorProvider`) outside it; `live.tsx` hands the pages their client,
   and no entry point calls `useClient` either.
   The CSP's `connect-src` names only the wallet's origin and the IC's API
   hosts, and `ic/client.ts` and `ic/facts.ts` are the only code that picks
   one.
5. **Every read is certified.** `ic/canister.ts` and `ic/ledger.ts` make
   their actors with `certified: true`, so reads go out as replicated calls
   the subnet certifies. Since only `ic/` may call `useClient`, no page can
   make an actor that reads otherwise. `ic/facts.ts` reads the IC's
   certified state of the wallet with an agent that verifies it.
6. **The prerendered HTML is the same for every visitor.** `app.tsx` renders
   nothing that needs the address, the network or the member until the page
   has hydrated (`parts/in-browser.ts`), so the HTML holds no private value
   and certifies once for everyone. The wallet's tests check every page
   holds no inline script or style, and CI checks the committed pages are
   the build's.
7. **The browser keeps as little as it can.** The claim page reads its key
   from the address's fragment and drops it from the address bar and
   history at once. Besides the sign-in session, which `@icp-sdk/auth` keeps
   in IndexedDB until it expires, the only thing kept is which audit entries
   were seen (`parts/seen.ts`), a convenience nothing depends on.

## What ships

8. **Every package in `app.js` is listed.** `ui/build.mjs` lists the
   packages the bundle may hold, and stops the build when it would hold one
   that is not listed, so a new dependency, or a new transitive one, ships
   only after someone has read it and added it. Each is pinned exactly in
   `ui/package-lock.json`.
9. **The pages are written in React and ship on Preact.** Lint refuses an
   import of Preact by name, so the code stays React's, and the build's
   aliases put Preact in React's place ([decision 0011](decisions/0011-preact-compat.md)).
   React is never in the package list, so the build stops if it would be
   bundled.
10. **The build gives the same bytes.** The pinned Node, esbuild and lockfile
    make the same files; CI builds twice, once from a clean copy in another
    directory, and compares them with the committed pages.

## Performance

11. **Every file has a budget.** The build stops when `app.js` is over
    600,000 bytes, `app.css` over 46,000 or a page over 20,000. A member
    downloads all of `app.js` on every visit, uncompressed unless the
    gateway compresses it, and the wallet serves a file of 2,000,000 bytes
    or more only in certified ranges. Raising a budget is a decision for the
    review of the change that needs it: `app.js` went from 560,000 to
    600,000 with the confirmed terms ([decision 0014](decisions/0014-confirmed-terms.md)).
12. **The page shows before its script runs.** Each page is its prerendered
    HTML, one stylesheet and one module script, which the browser runs after
    parsing the page. Nothing else loads; the next request is the app's
    first call to the wallet.
13. **Logic is plain TypeScript.** `logic/` takes what it needs as
    arguments, so the prerender, the browser and the tests run the same
    code, and `npm test` covers it in Node.

## Not covered yet

- **Trusted Types depend on the browser.** Chromium-based browsers enforce
  them; a browser that does not ignores the two directives, and rules 1 to 3
  rest on lint and the build there.
- **Cross-Origin-Opener-Policy.** `same-origin-allow-popups` would cut the
  page off from any window that opened it. It waits for a check that
  Internet Identity's sign-in window still answers through it.
- **Compressed files.** Certifying gzip or brotli encodings of `app.js`
  would cut what a member downloads to about a third.
- **The signed-in flows on Preact.** Signing in, sending, approving and the
  member forms have not yet run against a wallet in a browser.
