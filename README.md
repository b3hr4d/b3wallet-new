# b3wallet-new

A chain-key wallet that is one Internet Computer canister. Each wallet serves
its own certified UI, keeps its accounts and approval policy, signs with
threshold keys, is its own only controller, and upgrades itself only to a
release its owners approved.

**Status: step 14 of [the v0 plan](docs/plan-v0.md), the
[mainnet gate](docs/mainnet-gate.md).** The wallet canister
installs, keeps its state in stable memory across upgrades, answers `status`,
can be claimed once by its first owner, and shows its member list only to its
members. Members change members and the policy through actions that wait for
their quorum and timelock, a new device can ask to join, and every change
leaves one entry in an append-only audit log. It serves its own UI, certified
and with a strict Content-Security-Policy: a React app from `ui/`, prerendered
at build. Its start page shows the wallet's version and who you are to it,
shows members the accounts and their balances, lets owners add accounts, and
lets someone signed in who is not a member ask to join, as a viewer or, for
an AI agent through the MCP server in `mcp/`, as a proposer whose transfers
always wait for an owner
([decision 0007](docs/decisions/0007-agent-proposer.md)); its send page
proposes and runs a transfer, its approvals page approves, rejects and runs
the open actions, and its claim page lets the holder of the claim link sign
in with Internet Identity and claim the wallet
([decision 0006](docs/decisions/0006-client-library.md) chose ic-reactor 4 to
call it). It
holds ICP and ICRC tokens on its own principal and subaccounts, reads their
balances from the ledgers, and sends them by `Transfer` actions, at most once
each, with a daily limit below which a transfer needs no approvals. Its
Bitcoin accounts hold coins at addresses of its threshold ECDSA keys, and a
`Transfer` from one sends a transaction it signs with those keys through the
Bitcoin canister, again at most once. Releases are published to a release
channel canister, which serves a release only when two of its three signer
keys signed its manifest and its chunks match the manifest
([decision 0004](docs/decisions/0004-release-signers.md)); `release-tool`
makes and signs the manifests. A wallet installs a release on itself by an
`Upgrade` action, once two of the three signer keys it pins signed the release
and it was tested to upgrade from the version the wallet runs. It checks
first that it can pay for the install, and sends again an install the IC
refused. A release whose upgrade traps leaves the version before it running
and the upgrade marked `Failed`; nothing rescues a wallet from a release that
breaks upgrades
([decision 0005](docs/decisions/0005-rescue-path.md)). A factory canister
creates wallets: the cycles ledger takes a new wallet's cycles from its
creator's account, the factory installs the channel's release once two of
the three signer keys signed it, and the wallet becomes its own only
controller, with no rights left to the factory. Everything else is still
design, in [docs/design.md](docs/design.md).

## Principles

1. **One canister per wallet.** UI, API, policy and signer share one canister
   id and one domain.
2. **Owned by its users.** Whoever completes the claim link on the wallet's
   own domain becomes the first owner; owners add more principals later.
3. **Self-upgrading.** The canister is its only controller and installs
   approved, signed releases on itself.
4. **Everything certified.** Pages are prerendered at build and ship inside
   the signed release, the canister serves them certified, and private values
   come from calls the agent verifies in the browser.
5. **Extensions isolated.** Each extension is its own canister on its own
   origin, with its own sub-key and budget. The wallet's UI never renders
   extension code.

## Layout

| Path | What it is |
| --- | --- |
| `docs/design.md` | System design: architecture, flows, data model, security model, open decisions |
| `docs/plan-v0.md` | The v0 build, one small verified step per pull request |
| `candid/` | The target Candid interfaces of the wallet, release channel, factory and extensions |
| `canisters/wallet/` | The wallet canister; `pages/` holds the UI build it embeds |
| `ui/` | The wallet's UI: React pages prerendered at build, and the app that hydrates them |
| `mcp/` | An MCP server that lets an AI agent read a wallet and propose transfers, as a proposer whose transfers wait for an owner |
| `canisters/release_channel/` | The release channel canister |
| `canisters/factory/` | The factory canister, which creates wallets |
| `crates/release/` | Release manifests and their checks, shared by the channel, wallets and the tool |
| `tools/release-tool/` | The command-line tool that makes, signs and checks manifests |
| `tests/integration/` | PocketIC tests of the real wallet, release channel and factory Wasm |
| `tests/caller/` | A canister the PocketIC tests call through, to call as a canister |
| `scripts/try-local.sh`, `icp.yaml` | A local wallet to try, with icp-cli and a regtest Bitcoin node |
| `scripts/mainnet-gate.sh`, `docs/mainnet-gate.md` | The mainnet gate: deploy, upgrade a wallet two releases up, record the cycle costs |

## Development

The toolchain is pinned in `rust-toolchain.toml`. The Wasm build also compiles
secp256k1's C code, which needs a clang that compiles to WebAssembly. Apple's
cannot, so on a Mac install Homebrew's LLVM and point the build at it:

```sh
brew install llvm
export CC_wasm32_unknown_unknown="$(brew --prefix llvm)/bin/clang"
export AR_wasm32_unknown_unknown="$(brew --prefix llvm)/bin/llvm-ar"
```

CI runs the same commands:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p wallet -p release_channel -p factory -p caller --target wasm32-unknown-unknown --locked -- -D warnings
cargo test --workspace --exclude integration --locked -- --nocapture   # unit, interface and tool tests
cargo build -p wallet -p release_channel -p factory -p caller --target wasm32-unknown-unknown --release --locked
POCKET_IC_BIN=/path/to/pocket-ic ICRC1_LEDGER_WASM=/path/to/ic-icrc1-ledger.wasm.gz \
  BITCOIND=/path/to/bitcoind BITCOIN_CLI=/path/to/bitcoin-cli \
  cargo test -p integration --locked -- --include-ignored
```

The UI builds with the Node release `ui/package.json` names (the build checks
it) and the dependencies its lockfile pins. CI runs, in `ui/`:

```sh
npm ci
npm run build          # writes canisters/wallet/pages, which the wallet embeds
npm run format:check
npm run lint           # also refuses HTML strings, inline styles and eval
npm run typecheck
npm test
```

The wallet embeds the committed files in `canisters/wallet/pages`, so its own
build needs no Node; a change to `ui/` commits its build output with it. CI
checks the committed files are the build's, and builds again from a clean copy
in another directory to check it gives the same bytes.

The MCP server in `mcp/` runs on Node 22.18 or later; CI runs it on the UI's
Node. In `mcp/`, `npm ci` also writes its bindings, and CI then runs
`npm run format:check`, `npm run lint`, `npm run typecheck` and `npm test`.

The interface checks compare every method a canister exports with the same
method in its file in `candid/`; the wallet's prints the methods not
implemented yet. One release channel test publishes a release of 100 MiB, the
largest the channel takes. The
PocketIC tests need the server version and the ICRC-1 ledger release pinned in
`.github/workflows/ci.yml`; the ledger runs both as the ICP ledger, at its
mainnet id, and as an ICRC token. The Bitcoin tests start a regtest node of
their own with the Bitcoin Core release pinned there, and PocketIC's Bitcoin
canister follows it. The factory tests run on the cycles minting canister and
cycles ledger PocketIC installs itself. The upgrade test also needs
`PREVIOUS_WALLET_WASM`, the Wasm of the release before.

## Trying it locally

`scripts/try-local.sh` starts an icp-cli local network and a regtest Bitcoin
node, installs a new wallet, claims it with your current icp-cli identity, and
sends its accounts 10 local ICP and 50 regtest BTC. It then prints the calls
that check a balance and propose and execute a transfer. With `--browser` it
prints the new wallet's claim link instead, to open in a browser, sign in with
the network's own Internet Identity and claim the wallet there; you then add
its accounts on its start page, and it prints the commands that send them
local ICP and regtest BTC. It needs icp-cli and
ic-wasm (`npm install -g @icp-sdk/icp-cli @icp-sdk/ic-wasm`), Node, the Rust
toolchain, and on a Mac Homebrew's LLVM (`brew install llvm`), which it finds
on its own. Bitcoin Core (`brew install bitcoin`), whose `bitcoind` and
`bitcoin-cli` it finds on `PATH` or by `BITCOIND` and `BITCOIN_CLI`, adds the
Bitcoin account; without it the wallet gets only the ICP one.

```sh
./scripts/try-local.sh             # claimed by your icp-cli identity
./scripts/try-local.sh --browser   # claimed in the browser, from its claim link
```

Either way it also prints how to add the MCP server in `mcp/` to Claude Code
for the new wallet, so Claude can join it as a proposer, read it and propose
transfers that wait for your approval ([mcp/README.md](mcp/README.md)).

## Publishing a release

`release-tool` (`cargo run -p release-tool --`) never talks to the network: it
writes files that icp-cli sends. Each signer makes a key once and gives the
publisher its public key:

```sh
openssl genpkey -algorithm ed25519 -out signer.pem
release-tool public-key --key signer.pem
```

The release channel is installed with its publishers, the three public keys
and the threshold (`release-tool public-key --candid` prints a key as a Candid
blob). The icp-cli commands run in the repository root, whose `icp.yaml` names
the channel:

```sh
icp deploy release_channel --args '(variant { Init = record {
  publishers = vec { principal "<publisher>" };
  release_signers = vec { blob "..."; blob "..."; blob "..." };
  release_threshold = 2 : nat8;
} })'
```

A release, from its Wasm:

```sh
release-tool manifest --wasm wallet.wasm --version 0.2.0 \
  --notes-url https://example.org/0.2.0 --tested-from <module hash of 0.1.0> > manifest.did
release-tool sign --key signer.pem --wasm wallet.wasm manifest.did     # two signers, in turn
release-tool verify --signer <key 1> --signer <key 2> --signer <key 3> --threshold 2 \
  --wasm wallet.wasm manifest.did
release-tool chunks --wasm wallet.wasm --version 0.2.0 --out chunks
for f in chunks/chunk-*.bin; do
  icp canister call release_channel publish_chunk --args-file "$f" --args-format bin \
    --candid candid/release_channel.did
done
icp canister call release_channel publish_manifest --args-file manifest.did \
  --candid candid/release_channel.did
```

`sign` prints what it signs, the manifest's signing hash included, and with
`--wasm` first checks that the manifest names that Wasm. Signing hashes the
manifest's Candid encoding as the Rust `candid` crate writes it; other
encoders lay out the type table differently, so sign with this tool.

## Creating a wallet

The factory is installed with the release channel and the signer keys and
threshold every wallet it creates pins, the same as the channel's. In the
repository root, whose `icp.yaml` names the factory:

```sh
icp deploy factory --args '(variant { Init = record {
  release_channel = principal "<release channel>";
  release_signers = vec { blob "..."; blob "..."; blob "..." };
  release_threshold = 2 : nat8;
} })'
```

A creator pays with cycles from their account on the cycles ledger. They let
the factory take the wallet's cycles, at least 5T, and the ledger's fee of
100M, then call `create_wallet` with the public half of a fresh Ed25519 claim
key and when the claim expires, at least an hour away:

```sh
openssl genpkey -algorithm ed25519 -out claim.pem
release-tool public-key --candid --key claim.pem
icp canister call um5iw-rqaaa-aaaaq-qaaba-cai icrc2_approve '(record {
  spender = record { owner = principal "<factory>" };
  amount = 3_000_100_000_000 : nat;
})'
icp canister call factory create_wallet '(record {
  claim_public_key = blob "...";
  claim_expires_at_ns = <nanoseconds since 1970> : nat64;
  version = null;
  cycles = 3_000_000_000_000 : nat;
  from_subaccount = null;
})' --candid candid/factory.did
```

The factory checks the release's signatures before the ledger takes anything,
installs it, and answers with the wallet's id once the wallet is its own only
controller. Anyone can check that with `icp canister status <wallet>`, which
shows its controllers and module hash. Whoever holds the claim key then claims
it on the wallet's own page, from the claim link
`https://<wallet>.icp0.io/claim#<claim key>`, whose fragment is the key's
32-byte seed in base64url:

```sh
node -e 'const { createPrivateKey } = require("crypto");
  const key = createPrivateKey(require("fs").readFileSync("claim.pem"));
  console.log(key.export({ format: "der", type: "pkcs8" }).subarray(-32).toString("base64url"))'
```

The fragment never leaves the browser. If the release does not
install, the call fails with `InstallFailed` and the creator is the canister's
only controller, with the cycles left in it. `current_release` shows the
release a wallet created now gets.

## Upgrading a wallet

A wallet installs releases only on itself, so it must be its own controller.
One the factory created is; one installed by hand, as `scripts/try-local.sh`
installs it, needs the wallet added as a controller of itself. A member asks whether its channel has a release the wallet would
install, and an owner proposes it:

```sh
icp canister call wallet release_available '()' --candid candid/wallet.did
icp canister call wallet propose '(variant { Upgrade = record {
  version = "0.2.0"; wasm_sha256 = blob "..." } })' --candid candid/wallet.did
```

After the quorum and the timelock, `execute` stages the release's chunks in
the wallet's chunk store and sends the install. It can take several calls:
one that stops keeps the chunks staged so far, and one made while the wallet
waits for another call's answer stages everything but does not install.
`upgrade_status` (and `status`) shows `Staging`, `Installing`, `Idle` once
the new version runs, or `Failed` an hour after an install that did not run.

The IC holds back about 0.3T cycles for an install, so the wallet must be able
to spend 0.32T, plus 4B a chunk to stage, above what it keeps to stay running.
Short of that, `execute` stops with `InsufficientCycles`, which says what it
can spend and needs, and the action stays ready; anyone can top it up with
`icp canister top-up --amount 0.2t <wallet id>`. The IC also refuses an
install for a few seconds after another one, which the wallet's one-way call
never hears, so the wallet sends the install again 1, 3, 7, 15 and 31 minutes
later while it still runs the old version.

## Checking the schema

The interfaces in `candid/` compile with the
[candid-core](https://github.com/b3hr4d/candid-core) compiler:

```sh
# in a candid-core checkout
cargo build --release --bin candid-core --features filesystem-compiler --locked
for f in wallet release_channel factory extension; do
  target/release/candid-core compile path/to/b3wallet-new/candid/$f.did --no-source-info > /dev/null && echo "$f ok"
done
```
