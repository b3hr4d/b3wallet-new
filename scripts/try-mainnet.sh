#!/usr/bin/env bash
# Puts a wallet of this checkout on mainnet for you to try, without the factory
# or a release channel, on a subnet you choose, such as the SEV-SNP subnet
# re2t4-faa75-v3vhk-kdmdr-uyrkl-aik2l-ixd6u-p3fyr-zlfkc-6c5af-zae:
#
#   scripts/try-mainnet.sh install [--subnet SUBNET] [--cycles CYCLES]
#       creates the canister with 3T cycles (or CYCLES) from your icp-cli
#       identity's cycles ledger account, installs the wallet, and prints its
#       claim link, for you to claim it with Internet Identity in a browser
#   scripts/try-mainnet.sh upgrade
#       installs this checkout over the wallet, which keeps its state
#   scripts/try-mainnet.sh link
#       prints the claim link again
#
# Once you have claimed it, Settings > Guard uploads the guard file the wallet
# pins and loads it.
#
# Unlike a wallet from the factory, this one stays controlled by your icp-cli
# identity, not by itself: you can upgrade it in place, or stop and delete it
# to get its cycles back, and its settings page shows you as its controller.
# It trusts only a release signer key made here, which signs nothing, so it
# never upgrades itself. It holds real ICP and Bitcoin: keep to small amounts.
#
# -e ENVIRONMENT runs it elsewhere: with -e local it also claims the wallet
# with your icp-cli identity.
#
# Needs icp-cli, Node, curl, the Rust toolchain of rust-toolchain.toml, and a
# clang that compiles to WebAssembly (on a Mac, Homebrew's LLVM).
set -euo pipefail
cd "$(dirname "$0")/.."

usage() {
  echo "usage: $0 install [--subnet SUBNET] [--cycles CYCLES] [-e ENVIRONMENT]" >&2
  echo "       $0 upgrade [-e ENVIRONMENT]" >&2
  echo "       $0 link [-e ENVIRONMENT]" >&2
  exit 2
}
command=${1:-}
case $command in
  install | upgrade | link) shift ;;
  *) usage ;;
esac
env=ic subnet= cycles=3t
while [ $# -gt 0 ]; do
  case $1 in
    -e) env=${2:-}; [ -n "$env" ] || usage; shift 2 ;;
    --subnet) [ "$command" = install ] && [ -n "${2:-}" ] || usage; subnet=$2; shift 2 ;;
    --cycles) [ "$command" = install ] && [ -n "${2:-}" ] || usage; cycles=$2; shift 2 ;;
    *) usage ;;
  esac
done
net=(-e "$env")
did=(--candid candid/wallet.did)
state=.icp/try-mainnet/$env
target=target/wasm32-unknown-unknown/release

missing=()
need() { command -v "$1" >/dev/null 2>&1 || missing+=("  $1: $2"); }
need icp "npm install -g @icp-sdk/icp-cli"
need node "https://nodejs.org"
need cargo "https://rustup.rs"
need curl "your system's package manager"
# secp256k1's C code, under the bitcoin crate, needs a clang that compiles to
# WebAssembly and an ar that archives what it compiles. Apple's ar writes an
# empty archive of Wasm objects, even where Apple's clang compiles them, so on
# a Mac the build uses Homebrew's LLVM for both, as scripts/try-local.sh does.
compiles_wasm() { echo 'int x;' | "$1" --target=wasm32-unknown-unknown -x c -c - -o /dev/null 2>/dev/null; }
if [ -z "${CC_wasm32_unknown_unknown:-}" ] && { [ "$(uname)" = Darwin ] || ! compiles_wasm clang; }; then
  llvm=$(brew --prefix llvm 2>/dev/null)/bin || llvm=
  if [ -n "$llvm" ] && compiles_wasm "$llvm/clang"; then
    export CC_wasm32_unknown_unknown=$llvm/clang AR_wasm32_unknown_unknown=$llvm/llvm-ar
  else
    missing+=("  a clang and an ar that build WebAssembly (Apple's cannot): brew install llvm")
  fi
elif ! compiles_wasm "${CC_wasm32_unknown_unknown:-clang}"; then
  missing+=("  a clang that compiles to WebAssembly: brew install llvm")
fi
if [ ${#missing[@]} -gt 0 ]; then
  echo "Install these first:" >&2
  printf '%s\n' "${missing[@]}" >&2
  exit 1
fi

me=$(icp identity principal)
if [ "$me" = 2vxsx-fae ]; then
  echo "The anonymous identity cannot pay for or control a wallet. Make another one the default:" >&2
  echo "  icp identity new me && icp identity default me" >&2
  exit 1
fi
if [ "$env" = local ]; then
  page_of() { echo "http://$1.localhost:8000"; }
  icp network status >/dev/null 2>&1 || icp network start -d
else
  page_of() { echo "https://$1.icp0.io"; }
fi

# Ed25519 keys, as scripts/try-local.sh makes them: `new` writes a key and
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

# The subnet a canister runs on, as the IC's public API reports it, or nothing
# while the API does not list the canister: it answers 404 for a new one for
# some minutes.
subnet_of() {
  { curl -sSf "https://ic-api.internetcomputer.org/api/v3/canisters/$1" 2>/dev/null || true; } |
    node -e '
      let text = "";
      process.stdin.on("data", (d) => (text += d)).on("end", () => {
        try { console.log(JSON.parse(text).subnet_id ?? ""); } catch { console.log(""); }
      });'
}

claim_link() { echo "$(page_of "$1")/claim#$(ed25519 seed "$state/claim.pem")"; }

if [ "$command" = link ]; then
  if [ ! -e "$state/wallet" ] || [ ! -e "$state/claim.pem" ]; then
    echo "No wallet yet: run $0 install first." >&2
    exit 1
  fi
  claim_link "$(cat "$state/wallet")"
  exit 0
fi

build() {
  echo "Building the wallet"
  cargo build -q -p wallet --target wasm32-unknown-unknown --release --locked
}

if [ "$command" = upgrade ]; then
  if [ ! -e "$state/wallet" ]; then
    echo "No wallet yet: run $0 install first." >&2
    exit 1
  fi
  wallet=$(cat "$state/wallet")
  build
  icp canister install "$wallet" --mode upgrade --yes --wasm "$target/wallet.wasm" "${net[@]}" \
    --args '(variant { Upgrade = null })' >/dev/null
  echo "Wallet $wallet runs this checkout now: $(page_of "$wallet")/"
  exit 0
fi

if [ -e "$state/wallet" ]; then
  echo "Wallet $(cat "$state/wallet") is installed already: $0 upgrade installs this checkout over it." >&2
  echo "To pay for another one, remove $state first." >&2
  exit 1
fi
build
mkdir -p "$state"
on_subnet=()
[ -z "$subnet" ] || on_subnet=(--subnet "$subnet")
echo "Creating the canister with $cycles cycles from $me's cycles ledger account"
wallet=$(icp canister create --detached -q --cycles "$cycles" "${on_subnet[@]}" "${net[@]}")
echo "$wallet" >"$state/wallet"
# The claim key's private half lives only in the claim link's fragment.
expires=$(($(date +%s) + 24 * 60 * 60))000000000
icp canister install "$wallet" --wasm "$target/wallet.wasm" "${net[@]}" --args "(variant { Init = record {
  claim_public_key = blob \"$(ed25519 new "$state/claim.pem")\";
  claim_expires_at_ns = $expires : nat64;
  release_channel = principal \"ryjl3-tyaaa-aaaaa-aaaba-cai\";
  release_signers = vec { blob \"$(ed25519 new "$state/release.pem")\" };
  release_threshold = 1 : nat8;
  factory = null;
  bitcoin_network = $([ "$env" = local ] && echo 'opt variant { Regtest }' || echo null);
} })" >/dev/null
if [ -n "$subnet" ] && [ "$env" != local ]; then
  echo "Checking its subnet on ic-api.internetcomputer.org, which can take a few minutes to list it"
  landed=
  for _ in $(seq 20); do
    landed=$(subnet_of "$wallet")
    [ -z "$landed" ] || break
    sleep 15
  done
  if [ -n "$landed" ] && [ "$landed" != "$subnet" ]; then
    echo "Wallet $wallet runs on subnet $landed, not $subnet. To get its cycles back:" >&2
    echo "  icp canister stop $wallet -e $env && icp canister delete $wallet -e $env" >&2
    exit 1
  fi
  if [ -z "$landed" ]; then
    echo "The IC API does not list $wallet yet. Check its subnet later with:" >&2
    echo "  curl -s https://ic-api.internetcomputer.org/api/v3/canisters/$wallet" >&2
    subnet=
  fi
fi
page=$(page_of "$wallet")
if [ "$env" = local ]; then
  signature=$(ed25519 sign "$state/claim.pem" "claim:$wallet:$me")
  icp canister call "$wallet" claim "(record { signature = blob \"$signature\" })" "${did[@]}" "${net[@]}" >/dev/null
  echo
  echo "Wallet $wallet is installed and claimed by $me: $page/"
  exit 0
fi
cat <<DONE

Wallet $wallet is installed${subnet:+ on subnet $subnet}, controlled by your
icp-cli identity $me. Open its claim link within 24 hours, sign in with
Internet Identity, and claim it:

  $(claim_link "$wallet")

$0 link prints it again.

The link holds the claim key, so whoever opens it first owns the wallet.
Then Settings > Guard ($page/settings) loads the guard file the wallet pins.
To get the cycles back once you are done with it:

  icp canister stop $wallet -e $env && icp canister delete $wallet -e $env
DONE
