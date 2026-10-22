#!/usr/bin/env bash
# Starts a local network and a regtest Bitcoin node, installs a new wallet,
# claims it with your current icp-cli identity, and funds an ICP account and
# a Bitcoin account. Each run installs a new wallet in place of the last.
#
# With --browser it prints the wallet's claim link instead, for you to claim
# it in a browser with the local Internet Identity, and stops there.
#
# With --upgrade it upgrades the wallet it installed last to this checkout,
# in place: the wallet keeps its owners, accounts, balances and history.
#
# Needs icp-cli and ic-wasm, Node, the Rust toolchain of rust-toolchain.toml,
# and a clang that compiles to WebAssembly (on a Mac, Homebrew's LLVM).
# Bitcoin Core's bitcoind and bitcoin-cli, on PATH or
# named by BITCOIND and BITCOIN_CLI, add the Bitcoin account; without them
# the wallet gets only its ICP account.
set -euo pipefail
cd "$(dirname "$0")/.."

browser=false
upgrade=false
case "${1:-}" in
  --browser) browser=true ;;
  --upgrade) upgrade=true ;;
  "") ;;
  *) echo "usage: $0 [--browser | --upgrade]" >&2; exit 2 ;;
esac

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
# prints its public key, `sign` prints a signature, both as Candid blob text,
# and `seed` prints the key as a claim link's fragment holds it.
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
      : command === "seed"
      ? key.export({ format: "der", type: "pkcs8" }).subarray(-32).toString("base64url")
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

if $upgrade; then
  if ! wallet=$(icp canister status wallet --id-only 2>/dev/null); then
    echo "There is no wallet to upgrade: run $0 first." >&2
    exit 1
  fi
  # A wallet's Bitcoin network is set once it has a Bitcoin account; this
  # names it for a wallet from before the setting.
  icp deploy wallet --mode upgrade --yes \
    --args "(variant { Upgrade = opt record { bitcoin_network = opt variant { Regtest } } })"
  echo
  echo "Wallet $wallet runs this checkout now: http://$wallet.localhost:8000/"
  exit 0
fi

# The claim key's private half would live in the claim link's fragment.
expires=$(($(date +%s) + 24 * 60 * 60))000000000
icp deploy wallet --mode reinstall --cycles 20t --yes --args "(variant { Init = record {
  claim_public_key = blob \"$(ed25519 new "$state/claim.pem")\";
  claim_expires_at_ns = $expires : nat64;
  release_channel = principal \"ryjl3-tyaaa-aaaaa-aaaba-cai\";
  release_signers = vec { blob \"$(ed25519 new "$state/release.pem")\" };
  release_threshold = 1 : nat8;
  factory = null;
  bitcoin_network = opt variant { Regtest };
} })"
wallet=$(icp canister status wallet --id-only)
page=http://$wallet.localhost:8000

# How to let an AI agent in through mcp/, as a proposer whose transfers wait
# for an owner. $1 says where the owner approves.
agent() {
  cat <<DONE

To let Claude read the wallet and propose transfers, which wait for you,
set up its MCP server once and add it to Claude Code (mcp/README.md):

  (cd mcp && npm ci)
  claude mcp add b3wallet -e B3WALLET_ID=$wallet -e B3WALLET_NETWORK=local -- node "$PWD/mcp/src/main.ts"

Then ask Claude to join the wallet, and $1
DONE
}

if $browser; then
  cat <<DONE

Wallet $wallet waits for its first owner. Open its claim link, sign in with
the local Internet Identity, and claim it:

  $page/claim#$(ed25519 seed "$state/claim.pem")

The link holds the claim key, so whoever opens it first owns the wallet.
Its Internet Computer account is the wallet's own principal, and this sends
it 10 local ICP:

  icp token transfer 10 $wallet
DONE
  agent "approve its join request on $page/approvals once
the code there is the one Claude gives you. Approve each transfer it proposes
there too."
  if $bitcoin; then
    cat <<DONE

Its Bitcoin account's address is on its start page once it is yours, and
this mines 50 regtest BTC to it, spendable 100 blocks later:

  $bitcoin_cli -regtest -rpcport=18443 -rpcuser=local -rpcpassword=local generatetoaddress 1 <address>
  $bitcoin_cli -regtest -rpcport=18443 -rpcuser=local -rpcpassword=local generatetoaddress 100 $burn

Stop everything with: icp network stop; $bitcoin_cli -regtest -rpcport=18443 -rpcuser=local -rpcpassword=local stop
DONE
  else
    echo
    echo "Stop the network with: icp network stop"
  fi
  exit 0
fi

signature=$(ed25519 sign "$state/claim.pem" "claim:$wallet:$me")
icp canister call wallet claim "(record { signature = blob \"$signature\" })" "${did[@]}"

icp token transfer 10 "$wallet"
if $bitcoin; then
  # The wallet adds its Bitcoin account once the subnet gives it the key.
  for _ in $(seq 60); do
    bitcoin_address=$(icp canister call wallet accounts '()' "${did[@]}" \
      | sed -n 's/.*address = "\(bcrt1[a-z0-9]*\)";/\1/p' | head -n 1)
    [ -n "$bitcoin_address" ] && break
    sleep 1
  done
  [ -n "$bitcoin_address" ] || { echo "The wallet has no Bitcoin account yet." >&2; exit 1; }
  # A mining reward can be spent 100 blocks later.
  btc generatetoaddress 1 "$bitcoin_address" >/dev/null
  btc generatetoaddress 100 "$burn" >/dev/null
fi

cat <<DONE

Wallet $wallet is yours. Its Internet Computer account 1 received 10 local
ICP.
DONE
if $bitcoin; then
  cat <<DONE
Its Bitcoin account 2, at $bitcoin_address, received
50 regtest BTC; its balance shows them once the Bitcoin canister has the blocks.
DONE
fi
cat <<DONE
A wallet installed again at the same id keeps what its accounts hold. Its page,
where you sign in with the local Internet Identity: $page/

Call it with the interface file, which icp-cli cannot read from the canister;
propose returns the id to execute. Send 1 ICP back to yourself:

  icp canister call wallet balance '(1 : nat32, null)' ${did[*]}
  icp canister call wallet propose '(variant { Transfer = record {
    from = 1 : nat32; asset = null; to = "$me";
    amount = 100_000_000 : nat; fee = null; memo = null } })' ${did[*]}
  icp canister call wallet execute '(1 : nat64)' ${did[*]}
DONE
agent "approve and run its join request once its proposed_by
is the principal Claude gives you, and each transfer it proposes the same way:

  icp canister call wallet pending '()' ${did[*]}
  icp canister call wallet approve '(<action id> : nat64)' ${did[*]}
  icp canister call wallet execute '(<action id> : nat64)' ${did[*]}"
if $bitcoin; then
  cat <<DONE

Send 1 regtest BTC, then mine a block to confirm it:

  icp canister call wallet balance '(2 : nat32, null)' ${did[*]}
  icp canister call wallet propose '(variant { Transfer = record {
    from = 2 : nat32; asset = null; to = "$burn";
    amount = 100_000_000 : nat; fee = null; memo = null } })' ${did[*]}
  icp canister call wallet execute '(2 : nat64)' ${did[*]}
  $bitcoin_cli -regtest -rpcport=18443 -rpcuser=local -rpcpassword=local generatetoaddress 1 $burn

Stop everything with: icp network stop; $bitcoin_cli -regtest -rpcport=18443 -rpcuser=local -rpcpassword=local stop
DONE
else
  echo
  echo "Stop the network with: icp network stop"
fi
