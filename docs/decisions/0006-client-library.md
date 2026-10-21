# 0006: ic-reactor 4 is the UI's client library

Status: accepted, 2026-10-21. Behrad chose option A.

**Decision: the wallet's UI calls the wallet with ic-reactor 4
(`@ic-reactor/core` and `@ic-reactor/react`), pinned to the exact
4.0.0-beta.6 until 4.0.0 ships.**

Whatever the UI bundles ships inside the signed release, so each dependency
is code the release signers vouch for. The UI reads with TanStack Query's
`useQuery(client.queryOptions(...))` and writes with
`useMutation(client.mutationOptions(...))`, over bindings `candid-core-cli
gen` writes from `candid/wallet.did` on every build. Users sign in with
Internet Identity through `@icp-sdk/auth`'s `AuthClient`, which ic-reactor
drives.

| Option | What it is | For | Against |
| --- | --- | --- | --- |
| **A. ic-reactor 4** | `@ic-reactor/core` and `@ic-reactor/react` on TanStack Query, with Internet Identity sign-in | Typed calls from the interface file, cache keys per caller, writes refused after a switch of account, and errors that say whether a write may have run; the wallet also tests 4.0 before its release | A beta, and TanStack Query as an extra dependency |
| B. ic-reactor 3 | The stable 3.13.0 | Stable today | A migration to 4 later |
| C. Plain agent | `@icp-sdk/core` and `@icp-sdk/auth`, with small hooks of the wallet's own | The fewest dependencies | More code of our own to get right |

Every version is exact in `ui/package.json` and `ui/package-lock.json`, so a
release changes a dependency only through a reviewed lockfile change. Moving
from the beta to 4.0.0 is one such change.
