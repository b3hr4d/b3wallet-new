# 0011: The React UI ships on Preact

Status: accepted, 2026-10-24.

**Decision: the UI stays written in React, and its build bundles Preact in
React's place: `ui/build.mjs` aliases `react`, `react-dom` and their
subpaths to `preact/compat`, for the browser's bundle and the prerender
alike. Preact is pinned to the exact 11.0.1, and preact-render-to-string,
which renders the pages through `preact/compat/server`, to the exact 6.8.1.**

Whatever the UI bundles ships inside the signed release, and every member
downloads it on every visit: the wallet serves `app.js` uncompressed, so
unless the gateway compresses it the browser gets every byte. Of the React
build's 735 KiB of minified `app.js`, react-dom, react and scheduler were
222 KiB (30 %), more than anything but the IC client. Preact's runtime is
14 KiB. `preact/compat` gives Preact React's API, so the pages, ic-reactor's
React bindings ([decision 0006](0006-client-library.md)) and TanStack
Query's React adapter run on it with no change to their source. The pages
are still prerendered at build and hydrated in the browser
([decision 0001](0001-react-rendering.md)); only the renderer changed.

The numbers below come from building each option to the same spec with the
pinned Node 22.22.0 and esbuild 0.28.2, gzip at level 9:

| Option | What it is | For | Against |
| --- | --- | --- | --- |
| **A. React on `preact/compat`** | The same React source, built with aliases | `app.js` 735 KiB to 523 KiB, 227 KiB to 164 KiB gzipped (28 % smaller); no source change; `@ic-reactor/react`'s provider and `useAuth`, and TanStack's `useQuery`, run as they are; the same CSP and the same single `/app.js` | Where Preact differs from React (below) is ours to watch; React stays installed for types and peers, so the lockfile holds both |
| B. Keep React 19.3.1 | The UI as decisions 0001 and 0006 left it | What ic-reactor's React bindings are written and tested against | 222 KiB of every download is the renderer |
| C. Port the UI to another TypeScript framework | Preact's own API, Lit, Solid, Vue or Svelte, each with TanStack Query's adapter for it | Preact's port is the smallest, 161 KiB gzipped, 3 KiB below A | Every page rewritten; none can use `@ic-reactor/react`, so sign-in and the provider become code of our own |
| D. A Rust framework compiled to Wasm | Leptos, Dioxus, Yew, Sycamore, Sauron or Dominator, calling the wallet with ic-agent | One language with the canister | 3.7 to 5.8 times React's gzipped size; needs `'wasm-unsafe-eval'` in the CSP; about a minute to build; cannot use ic-reactor |

How it is built, and what checks it:

- The prerender now bundles its packages instead of importing them from
  `node_modules`, so the aliases reach the React that `@ic-reactor/react`
  and `@tanstack/react-query` import, and one Preact renders everything.
  Left external, those packages would load React itself in Node, whose
  hooks work only under React's renderer.
- React 19.3.1, react-dom and their types stay installed: TypeScript checks
  the pages against React's types, and they are the peers ic-reactor's and
  TanStack's React packages declare. The build never bundles them.
- What the pages could notice: Preact passes the DOM's own events, not
  React's synthetic ones, and `preact/compat` maps `onChange` on a text
  input to its `input` event, so it still fires on every keystroke, as in
  React. A dependency that reached into react-dom's internals would break;
  none of those bundled today does.
- Format, lint, typecheck and the UI's 62 tests pass; the build gives the
  same bytes twice, and from a clean copy in another directory after
  `npm ci`. Each prerendered page differs from React's only in the order of
  attributes on some elements, and `app.css` is unchanged. The wallet's
  HTTP tests and its other unit tests pass with the new pages embedded.
- In a browser, at a plain localhost the pages hydrate with no console
  error; at a canister address with no replica behind it, ic-reactor's
  bindings start on Preact and draw the sign-in button, the certified
  status read and the health pane. Not checked yet: signing in, sending,
  approving and the member forms against a running wallet.
