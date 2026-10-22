#!/usr/bin/env bash
# Builds the recovery controller's Wasm (decision 0009) so that anyone who
# builds the same commit with the toolchain of rust-toolchain.toml gets the
# same bytes, and prints their SHA-256: the module hash owners check before
# they hand a wallet to a controller. The paths of this checkout and of
# Cargo's home would end up in the Wasm's panic messages, and differ from
# one computer to the next, so the build maps them to fixed names.
#
# Usage: scripts/build-controller.sh
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cargo_home=$(cd "${CARGO_HOME:-$HOME/.cargo}" && pwd)
export RUSTFLAGS="--remap-path-prefix=$root=/b3wallet --remap-path-prefix=$cargo_home=/cargo"
# Its own target directory: other flags would rebuild everything else.
export CARGO_TARGET_DIR="$root/target/controller-build"

cargo build --manifest-path "$root/Cargo.toml" -p controller \
  --target wasm32-unknown-unknown --release --locked >&2
wasm="$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/controller.wasm"
if grep -q -a -F -e "$root" -e "$cargo_home" "$wasm"; then
  echo "error: a build path is still in $wasm" >&2
  exit 1
fi
sha256sum "$wasm"
