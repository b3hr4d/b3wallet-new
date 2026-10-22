# v0 build plan

v0 is one owner, one wallet canister, on mainnet: claim link, Internet Identity
sign-in, ICP, ICRC and Bitcoin accounts, transfers, certified pages, and signed
releases installed by a chunked self-upgrade. It is done when a chunked
self-upgrade passes from v(n) to v(n+2) on mainnet and its cycle cost is
measured.

Each step below is one pull request. A step is merged only when its checks pass
in CI, and each step's tests include the failure cases, not only the happy path.

## Tooling

- Rust stable, `ic-cdk` 0.20, `ic-stable-structures`, `ic-http-certification`
  4.0, `candid`.
- Integration tests run the real canister Wasm in PocketIC (`pocket-ic` crate,
  server binary pinned in CI).
- `icp-cli` for local and mainnet deploys.
- CI on every pull request: `cargo fmt --check`, `cargo clippy -- -D warnings`,
  unit tests, the Wasm build, PocketIC tests, and the interface check below.

## Interface check

`candid/wallet.did` is the target. From step 1, a test exports the canister's
own interface and checks every method it implements against the same method in
`candid/wallet.did`, type for type. Methods not implemented yet are listed by
the test, so the gap is always visible.

## Steps

1. **Workspace and CI.** Cargo workspace, `wallet` crate building to
   `wasm32-unknown-unknown`, a `status` query, a PocketIC smoke test, the
   interface check. Done when CI is green and a deliberately wrong signature
   fails the interface check.
2. **Stable storage.** The memory layout (one virtual memory per store, ids
   recorded in `docs/design.md`), config and members stores, versioned Candid
   values. Done when a PocketIC test writes state, upgrades the canister, and
   reads the same state back.
3. **Claim.** Ed25519 claim key from `InitArgs`, the signed message
   `claim:<wallet id>:<caller principal>`, expiry, one use, and the rule that an
   unclaimed wallet answers only `claim`, `status` and `http_request`. Done when
   tests cover a wrong principal, a replayed signature, an expired key and a
   second claim.
4. **Members.** Roles, `whoami`, `members`, and caller guards on every method.
   Done when a non-member is refused by every non-public method.
   (`request_join` moved to step 5: it creates a pending action.)
5. **Policy and pending actions.** A pure Rust policy engine (quorum, daily
   limits, timelocks, expiry) with property tests, then `propose`, `approve`,
   `reject`, `execute`, `pending` and `request_join`. Done when PocketIC tests
   advance time through a timelock and an expiry.
6. **Audit log.** An append-only log written by every privileged method. Done
   when each action kind leaves exactly one entry per state change.
7. **Certified HTTP.** `http_request` for files embedded in the Wasm with
   `include_bytes!` (a static HTML page first, no React yet), certified with
   `ic-asset-certification` in `init` and `post_upgrade`, the certified 404,
   and the CSP header from decision 0001. Done when every response verifies
   with `ic-response-verification` in tests, also after an upgrade, and a test
   pins the exact CSP header.
8. **ICP and ICRC accounts.** Accounts on the wallet's own principal and
   subaccounts, `balance`, and `Transfer` actions against a real ICRC-1 ledger
   Wasm in PocketIC.
9. **Signer and Bitcoin.** Threshold ECDSA by derivation path
   (`wallet / chain / index`), Bitcoin addresses, UTXO selection and signed
   transactions on regtest.
10. **Release channel.** The release channel canister and a command-line tool
    that signs manifests. Done when a manifest with too few signatures or a
    wrong chunk hash is refused.
11. **Self-upgrade.** `release_available`, the `Upgrade` action, chunk staging
    with `upload_chunk`, the one-way `install_chunked_code`, and failure
    detection. Done when PocketIC upgrades v(n) to v(n+1) to v(n+2), and a
    release whose `post_upgrade` traps leaves the old version running and
    marked `Failed`.
12. **Factory.** `create_wallet`: create, install a signed release, hand
    control to the wallet, drop itself. Done when the factory is no longer a
    controller of any wallet it created.
13. **UI.** The React app, prerendered at build into page shells and hydrated
    in the browser, with every private value from agent calls. The build is
    reproducible (pinned Node, esbuild and lockfile) and its output is
    embedded in the release; a lint rule bans `dangerouslySetInnerHTML`. The
    client library choice is made at this step. Done when two clean builds
    produce identical bytes and every served page verifies.
14. **Mainnet gate.** Deploy, run the chunked self-upgrade from v(n) to v(n+2)
    on mainnet, and record the cycle cost of each step.

15. **AI agents as proposers.** A `Proposer` role whose approval never
    counts, and an MCP server in `mcp/` with its own key, so an AI on the
    owner's computer can read the wallet and propose transfers that wait for
    an owner ([decision 0007](decisions/0007-agent-proposer.md)). Done when a
    proposer's transfer under the daily limit still waits for an owner, and
    an AI client proposes one through the server on a local network.

## Decisions still open

- License.
- Who holds the three release signer keys (before step 14).
