# b3wallet-new

A chain-key wallet that is one Internet Computer canister. Each wallet serves
its own certified UI, keeps its accounts and approval policy, signs with
threshold keys, is its own only controller, and upgrades itself only to a
release its owners approved.

**Status: step 1 of [the v0 plan](docs/plan-v0.md).** The wallet canister
installs and answers `status`; everything else is still design, in
[docs/design.md](docs/design.md).

## Principles

1. **One canister per wallet.** UI, API, policy and signer share one canister
   id and one domain.
2. **Owned by its users.** Whoever completes the claim link on the wallet's
   own domain becomes the first owner; owners add more principals later.
3. **Self-upgrading.** The canister is its only controller and installs
   approved, signed releases on itself.
4. **Everything certified.** Pages are rendered and certified when data
   changes and served from a query, so the gateway verifies every byte.
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

## Development

The toolchain is pinned in `rust-toolchain.toml`. CI runs the same commands:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p wallet --target wasm32-unknown-unknown --locked -- -D warnings
cargo test -p wallet --locked -- --nocapture   # unit tests and the interface check
cargo build -p wallet --target wasm32-unknown-unknown --release --locked
POCKET_IC_BIN=/path/to/pocket-ic cargo test -p integration --locked
```

The interface check compares every method the canister exports with the same
method in `candid/wallet.did` and prints the methods not implemented yet. The
PocketIC tests need the server version pinned in `.github/workflows/ci.yml`.

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
