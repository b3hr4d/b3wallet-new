# b3wallet-new

A chain-key wallet that is one Internet Computer canister. Each wallet serves
its own certified UI, keeps its accounts and approval policy, signs with
threshold keys, is its own only controller, and upgrades itself only to a
release its owners approved.

**Status: step 9 of [the v0 plan](docs/plan-v0.md).** The wallet canister
installs, keeps its state in stable memory across upgrades, answers `status`,
can be claimed once by its first owner, and shows its member list only to its
members. Members change members and the policy through actions that wait for
their quorum and timelock, a new device can ask to join, and every change
leaves one entry in an append-only audit log. It serves a placeholder start
page and a 404, both certified and with a strict Content-Security-Policy. It
holds ICP and ICRC tokens on its own principal and subaccounts, reads their
balances from the ledgers, and sends them by `Transfer` actions, at most once
each, with a daily limit below which a transfer needs no approvals. Its
Bitcoin accounts hold coins at addresses of its threshold ECDSA keys, and a
`Transfer` from one sends a transaction it signs with those keys through the
Bitcoin canister, again at most once. Everything else is still design, in
[docs/design.md](docs/design.md).

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
| `canisters/wallet/` | The wallet canister |
| `tests/integration/` | PocketIC tests of the real wallet Wasm |
| `scripts/try-local.sh`, `icp.yaml` | A local wallet to try, with icp-cli and a regtest Bitcoin node |

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
cargo clippy -p wallet --target wasm32-unknown-unknown --locked -- -D warnings
cargo test -p wallet --locked -- --nocapture   # unit tests and the interface check
cargo build -p wallet --target wasm32-unknown-unknown --release --locked
POCKET_IC_BIN=/path/to/pocket-ic ICRC1_LEDGER_WASM=/path/to/ic-icrc1-ledger.wasm.gz \
  BITCOIND=/path/to/bitcoind BITCOIN_CLI=/path/to/bitcoin-cli \
  cargo test -p integration --locked -- --include-ignored
```

The interface check compares every method the canister exports with the same
method in `candid/wallet.did` and prints the methods not implemented yet. The
PocketIC tests need the server version and the ICRC-1 ledger release pinned in
`.github/workflows/ci.yml`; the ledger runs both as the ICP ledger, at its
mainnet id, and as an ICRC token. The Bitcoin tests start a regtest node of
their own with the Bitcoin Core release pinned there, and PocketIC's Bitcoin
canister follows it. The upgrade test also needs `PREVIOUS_WALLET_WASM`, the
Wasm of the release before.

## Trying it locally

`scripts/try-local.sh` starts an icp-cli local network and a regtest Bitcoin
node, installs a new wallet, claims it with your current icp-cli identity, and
sends its accounts 10 local ICP and 50 regtest BTC. It then prints the calls
that check a balance and propose and execute a transfer. It needs icp-cli and
ic-wasm (`npm install -g @icp-sdk/icp-cli @icp-sdk/ic-wasm`), Node, the Rust
toolchain, and on a Mac Homebrew's LLVM (`brew install llvm`), which it finds
on its own. Bitcoin Core (`brew install bitcoin`), whose `bitcoind` and
`bitcoin-cli` it finds on `PATH` or by `BITCOIND` and `BITCOIN_CLI`, adds the
Bitcoin account; without it the wallet gets only the ICP one.

```sh
./scripts/try-local.sh
```

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
