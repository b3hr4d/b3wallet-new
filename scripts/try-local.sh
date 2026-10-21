#!/usr/bin/env bash
# Starts a local network and a regtest Bitcoin node, installs a new wallet,
# claims it with your current icp-cli identity, and funds an ICP account and
# a Bitcoin account. Each run installs a new wallet in place of the last.
#
# Needs icp-cli and ic-wasm, Node, the Rust toolchain of rust-toolchain.toml,
# and a clang that compiles to WebAssembly (on a Mac, Homebrew's LLVM).
# Bitcoin Core's bitcoind and bitcoin-cli, on PATH or
# named by BITCOIND and BITCOIN_CLI, add the Bitcoin account; without them
# the wallet gets only its ICP account.
set -euo pipefail
cd "$(dirname "$0")/.."

state=.icp/try
bitcoind=${BITCOIND:-bitcoind}
bitcoin_cli=${BITCOIN_CLI:-bitcoin-cli}
btc() { "$bitcoin_cli" -regtest -rpcport=18443 -rpcuser=local -rpcpassword=local "$@"; }
did=(--candid candid/wallet.did)
# Mining rewards go to BIP-173's example address, whose key nobody holds.
burn=bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080

missing=()
need() { command -v "$1" >/dev/null 2>&1 || missing+=("  $1: $2"); }
need icp "npm install -g @icp-sdk/icp-cli"
need ic-wasm "npm install -g @icp-sdk/ic-wasm"
need node "https://nodejs.org"
need cargo "https://rustup.rs"
# secp256k1's C code, under the bitcoin crate, needs a clang that compiles to
# WebAssembly. Apple's cannot, so on a Mac the build uses Homebrew's LLVM.
compiles_wasm() { echo 'int x;' | "$1" --target=wasm32-unknown-unknown -x c -c - -o /dev/null 2>/dev/null; }
if ! compiles_wasm "${CC_wasm32_unknown_unknown:-clang}"; then
  llvm=$(brew --prefix llvm 2>/dev/null)/bin || llvm=
  if [ -z "${CC_wasm32_unknown_unknown:-}" ] && [ -n "$llvm" ] && compiles_wasm "$llvm/clang"; then
    export CC_wasm32_unknown_unknown=$llvm/clang AR_wasm32_unknown_unknown=$llvm/llvm-ar
  else
    missing+=("  a clang that compiles to WebAssembly (Apple's cannot): brew install llvm")
  fi
fi
# The script's calls name the interface file, which icp-cli 0.3.2 added.
if command -v icp >/dev/null 2>&1; then
  case $(icp canister call --help 2>&1) in
    *--candid*) ;;
    *) missing+=("  icp-cli 0.3.2 or later: npm install -g @icp-sdk/icp-cli@latest") ;;
  esac
fi
if [ ${#missing[@]} -gt 0 ]; then
  echo "Install these first:" >&2
  printf '%s\n' "${missing[@]}" >&2
  exit 1
fi
if command -v "$bitcoind" >/dev/null 2>&1 && command -v "$bitcoin_cli" >/dev/null 2>&1; then
  bitcoin=true
else
  bitcoin=false
  echo "Bitcoin Core is not installed, so the wallet gets no Bitcoin account."
  echo "For one, install it (brew install bitcoin, or https://bitcoincore.org/en/download/) and run this again."
fi

me=$(icp identity principal)
if [ "$me" = 2vxsx-fae ]; then
  echo "The anonymous identity cannot claim a wallet. Make another one the default:" >&2
  echo "  icp identity new me && icp identity default me" >&2
  exit 1
fi

# Ed25519 keys for the claim and the release signer: `new` writes a key and
# prints its public key, `sign` prints a signature, both as Candid blob text.
ed25519() {
  node -e '
    const crypto = require("crypto"), fs = require("fs");
    const [command, pem, message] = process.argv.slice(1);
    const blob = (bytes) => [...bytes].map((b) => "\\" + b.toString(16).padStart(2, "0")).join("");
    if (command === "new") {
      const { privateKey } = crypto.generateKeyPairSync("ed25519");
      fs.writeFileSync(pem, privateKey.export({ format: "pem", type: "pkcs8" }), { mode: 0o600 });
    }
    const key = crypto.createPrivateKey(fs.readFileSync(pem));
    console.log(command === "sign"
      ? blob(crypto.sign(null, Buffer.from(message), key))
      : blob(crypto.createPublicKey(key).export({ format: "der", type: "spki" }).subarray(-32)));
  ' "$@"
}

# icp.yaml pins the network launcher. ICP_CLI_NETWORK_LAUNCHER_PATH would run
# another one in its place, and icp-cli cannot read canister settings from an
# older one, so the script drops it and stops a network it may have started.
if [ -n "${ICP_CLI_NETWORK_LAUNCHER_PATH:-}" ]; then
  echo "Ignoring ICP_CLI_NETWORK_LAUNCHER_PATH: this runs the network launcher icp.yaml pins."
  unset ICP_CLI_NETWORK_LAUNCHER_PATH
  icp network stop >/dev/null 2>&1 || true
fi

mkdir -p "$state/bitcoin"
if $bitcoin && ! btc getblockcount >/dev/null 2>&1; then
  echo "Starting a regtest Bitcoin node in $state/bitcoin"
  "$bitcoind" -regtest -daemon -datadir="$state/bitcoin" -port=18444 -rpcport=18443 \
    -rpcuser=local -rpcpassword=local -bind=127.0.0.1 -rpcbind=127.0.0.1 -rpcallowip=127.0.0.1
  until btc getblockcount >/dev/null 2>&1; do sleep 1; done
fi
icp network status >/dev/null 2>&1 || icp network start -d

# The claim key's private half would live in the claim link's fragment.
expires=$(($(date +%s) + 24 * 60 * 60))000000000
icp deploy wallet --mode reinstall --cycles 20t --yes --args "(variant { Init = record {
  claim_public_key = blob \"$(ed25519 new "$state/claim.pem")\";
  claim_expires_at_ns = $expires : nat64;
  release_channel = principal \"ryjl3-tyaaa-aaaaa-aaaba-cai\";
  release_signers = vec { blob \"$(ed25519 new "$state/release.pem")\" };
  release_threshold = 1 : nat8;
  factory = null;
} })"
wallet=$(icp canister status wallet --id-only)

signature=$(ed25519 sign "$state/claim.pem" "claim:$wallet:$me")
icp canister call wallet claim "(record { signature = blob \"$signature\" })" "${did[@]}"

# Adds an account and prints its address.
account() {
  icp canister call wallet add_account "(record { chain = variant { $1 }; label = \"$2\" })" "${did[@]}" \
    | sed -n 's/.*address = "\(.*\)";/\1/p'
}
account Icp icp >/dev/null
icp token transfer 10 "$wallet"
if $bitcoin; then
  bitcoin_address=$(account "Bitcoin = variant { Regtest }" bitcoin)
  # A mining reward can be spent 100 blocks later.
  btc generatetoaddress 1 "$bitcoin_address" >/dev/null
  btc generatetoaddress 100 "$burn" >/dev/null
fi

cat <<DONE

Wallet $wallet is yours. Its ICP account 1 received 10 local ICP.
DONE
if $bitcoin; then
  cat <<DONE
Its Bitcoin account 2, at $bitcoin_address, received
50 regtest BTC; its balance shows them once the Bitcoin canister has the blocks.
DONE
fi
cat <<DONE
A wallet installed again at the same id keeps what its accounts hold. Its page,
a placeholder until the UI step: http://wallet.local.localhost:8000/

Call it with the interface file, which icp-cli cannot read from the canister;
propose returns the id to execute. Send 1 ICP back to yourself:

  icp canister call wallet balance '(1 : nat32)' ${did[*]}
  icp canister call wallet propose '(variant { Transfer = record {
    from = 1 : nat32; to = "$me";
    amount = 100_000_000 : nat; fee = null; memo = null } })' ${did[*]}
  icp canister call wallet execute '(1 : nat64)' ${did[*]}
DONE
if $bitcoin; then
  cat <<DONE

Send 1 regtest BTC, then mine a block to confirm it:

  icp canister call wallet balance '(2 : nat32)' ${did[*]}
  icp canister call wallet propose '(variant { Transfer = record {
    from = 2 : nat32; to = "$burn";
    amount = 100_000_000 : nat; fee = null; memo = null } })' ${did[*]}
  icp canister call wallet execute '(2 : nat64)' ${did[*]}
  $bitcoin_cli -regtest -rpcport=18443 -rpcuser=local -rpcpassword=local generatetoaddress 1 $burn

Stop everything with: icp network stop; $bitcoin_cli -regtest -rpcport=18443 -rpcuser=local -rpcpassword=local stop
DONE
else
  echo
  echo "Stop the network with: icp network stop"
fi
