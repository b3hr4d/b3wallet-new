#!/usr/bin/env bash
# The mainnet gate, step 14 of docs/plan-v0.md: deploys a release channel and
# a factory, creates a wallet through the factory and claims it, then has the
# wallet upgrade itself two releases up, recording what each step costs in
# cycles. A wallet runs an upgrade only after its 7-day timelock, so the gate
# runs in two halves:
#
#   scripts/mainnet-gate.sh start    deploys, creates and claims the wallet,
#                                    publishes the next two releases and
#                                    proposes both upgrades
#   scripts/mainnet-gate.sh finish   7 to 14 days later, runs both upgrades
#
# Both halves add their rows to .icp/gate/<environment>/costs.md and print it.
# They spend cycles from your icp-cli identity's account on the cycles ledger,
# about 7.1T in all; `icp cycles mint --cycles 7.1t -e ic` buys them with ICP.
# With -e local the gate runs on the local network instead.
#
# The releases are the wallet's own Wasm, stamped with a public metadata
# section, b3wallet:release, that names them, and signed by three keys made
# for the gate. The gate's channel, factory and wallet trust only those keys,
# so the gate's wallet never installs a real release.
#
# Needs icp-cli and ic-wasm, Node, the Rust toolchain of rust-toolchain.toml,
# and a clang that compiles to WebAssembly (on a Mac, Homebrew's LLVM).
set -euo pipefail
cd "$(dirname "$0")/.."

usage() {
  echo "usage: $0 start|finish [-e ENVIRONMENT]" >&2
  exit 2
}
half=${1:-}
case $half in
  start | finish) shift ;;
  *) usage ;;
esac
env=ic
while [ $# -gt 0 ]; do
  case $1 in
    -e) env=${2:-}; [ -n "$env" ] || usage; shift 2 ;;
    *) usage ;;
  esac
done

state=.icp/gate/$env
costs=$state/costs.md
net=(-e "$env")
versions=(gate-1 gate-2 gate-3)
cycles_ledger=um5iw-rqaaa-aaaaq-qaaba-cai
# Cycles for each new canister, of which creating it costs 0.5T. The wallet
# gets the factory's minimum.
channel_cycles=1t
factory_cycles=1t
wallet_cycles=5_000_000_000_000
needed=7_100_000_000_000
target=target/wasm32-unknown-unknown/release

missing=()
need() { command -v "$1" >/dev/null 2>&1 || missing+=("  $1: $2"); }
need icp "npm install -g @icp-sdk/icp-cli"
need ic-wasm "npm install -g @icp-sdk/ic-wasm"
need node "https://nodejs.org"
need cargo "https://rustup.rs"
need curl "your system's package manager"
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
if [ ${#missing[@]} -gt 0 ]; then
  echo "Install these first:" >&2
  printf '%s\n' "${missing[@]}" >&2
  exit 1
fi

if [ "$env" = local ]; then
  page_of() { echo "http://$1.localhost:8000"; }
else
  page_of() { echo "https://$1.icp0.io"; }
fi

release_tool() { cargo run -q --locked -p release-tool -- "$@"; }

# Runs a command with its output in the gate's log, which shows on a failure.
logged() {
  if ! "$@" >>"$state/log" 2>&1; then
    echo "This failed, with the end of $state/log below: $*" >&2
    tail -n 20 "$state/log" >&2
    exit 1
  fi
}

# A time in nanoseconds since the epoch, in the local time zone.
date_text() {
  node -e '
    const options = { dateStyle: "medium", timeStyle: "short" };
    const date = new Date(Number(BigInt(process.argv[1]) / 1_000_000n));
    const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
    console.log(date.toLocaleString("en-GB", options) + " " + zone);
  ' -- "$1"
}

# A number of cycles without its digit separators.
digits() { tr -d '_' | grep -o '^[0-9]*'; }

# Cycles as a person reads them, with a sign: -1.500T, +520.4B.
cycles_text() {
  node -e '
    const n = BigInt(process.argv[1]);
    const a = n < 0n ? -n : n;
    const [unit, exponent] = a >= 10n ** 12n ? ["T", 12] : a >= 10n ** 9n ? ["B", 9] : ["M", 6];
    const sign = n < 0n ? "-" : "+";
    console.log(n === 0n ? "0" : sign + (Number(a) / 10 ** exponent).toFixed(3) + unit);
  ' -- "$1"
}

# A number of seconds as a person reads them: 45s, 3m 20s, 7d 2h.
duration_text() {
  local s=$1
  if [ "$s" -ge 86400 ]; then
    echo "$((s / 86400))d $((s % 86400 / 3600))h"
  elif [ "$s" -ge 3600 ]; then
    echo "$((s / 3600))h $((s % 3600 / 60))m"
  elif [ "$s" -ge 60 ]; then
    echo "$((s / 60))m $((s % 60))s"
  else
    echo "${s}s"
  fi
}

# The balances of whoever pays: your account on the cycles ledger, and the
# channel, factory and wallet once they exist. The wallet controls itself, so
# its own status method reports its balance.
balance_of() {
  case $1 in
    you) icp cycles balance -q "${net[@]}" | digits ;;
    channel | factory)
      icp canister status "${!1}" "${net[@]}" --json |
        node -pe 'JSON.parse(require("fs").readFileSync(0, "utf8")).cycles' | digits
      ;;
    wallet)
      icp canister call "$wallet" status '()' --query --candid candid/wallet.did "${net[@]}" |
        sed -n 's/.*cycles = \([0-9_]*\) : nat;.*/\1/p' | digits
      ;;
  esac
}

# Adds a row to the costs: what each payer's balance changed by since the
# last row, and how long the step took.
record() {
  local step=$1 row payer now before
  row="| $step | $(duration_text $(($(date +%s) - step_started))) |"
  for payer in you channel factory wallet; do
    before=before_$payer
    if [ "$payer" = you ] || [ -n "${!payer:-}" ]; then
      now=$(balance_of "$payer")
      row+=" $(cycles_text $((now - ${!before:-0}))) |"
      printf -v "$before" %s "$now"
    else
      row+=" |"
    fi
  done
  echo "$row" >>"$costs"
  echo "$row"
  step_started=$(date +%s)
  save
}

save() {
  {
    for name in me channel factory wallet hash_1 hash_2 hash_3 action_2 action_3 \
      runs_after_ns expires_at_ns step_started before_you before_channel before_factory \
      before_wallet; do
      [ -z "${!name:-}" ] || echo "$name=${!name}"
    done
  } >"$state/state.env"
}

# Ed25519 keys: `new` writes a key and prints its public key, `sign` prints a
# signature, both as Candid blob text, and `seed` prints the key as a claim
# link's fragment holds it.
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

# A SHA-256 in hex as a Candid blob.
hex_blob() { sed 's/../\\&/g'; }

# Builds release N's Wasm from the wallet's, and its manifest signed by the
# first two gate keys, then publishes both to the channel.
publish() {
  local n=$1 version=${versions[$1 - 1]} wasm=$state/wallet-${versions[$1 - 1]}.wasm
  local tested_from=() chunk
  ic-wasm "$target/wallet.wasm" -o "$wasm" metadata b3wallet:release -d "$version" -v public -k
  printf -v "hash_$n" %s "$(node -e '
    console.log(require("crypto").createHash("sha256").update(require("fs").readFileSync(process.argv[1])).digest("hex"))
  ' "$wasm")"
  if [ "$n" -gt 1 ]; then
    local previous=hash_$((n - 1))
    tested_from=(--tested-from "${!previous}")
  fi
  logged sh -c 'exec "$@" >"$0"' "$state/manifest-$version.did" \
    cargo run -q --locked -p release-tool -- manifest --wasm "$wasm" --version "$version" \
    --notes-url https://github.com/b3hr4d/b3wallet-new/blob/main/docs/mainnet-gate.md \
    "${tested_from[@]}"
  logged release_tool sign --key "$state/signer-1.pem" --wasm "$wasm" "$state/manifest-$version.did"
  logged release_tool sign --key "$state/signer-2.pem" --wasm "$wasm" "$state/manifest-$version.did"
  rm -rf "$state/chunks-$version"
  logged release_tool chunks --wasm "$wasm" --version "$version" --out "$state/chunks-$version"
  for chunk in "$state/chunks-$version"/chunk-*.bin; do
    logged icp canister call "$channel" publish_chunk --args-file "$chunk" --args-format bin \
      --candid candid/release_channel.did "${net[@]}"
  done
  logged icp canister call "$channel" publish_manifest --args-file "$state/manifest-$version.did" \
    --candid candid/release_channel.did "${net[@]}"
  chunks=$(ls "$state/chunks-$version" | wc -l | tr -d ' ')
}

# Proposes the upgrade to release N and prints its action id, when it can run
# and when it expires.
propose_upgrade() {
  local version=${versions[$1 - 1]} hash=hash_$1
  icp canister call "$wallet" propose "(variant { Upgrade = record {
    version = \"$version\";
    wasm_sha256 = blob \"$(echo "${!hash}" | hex_blob)\";
  } })" --candid candid/wallet.did "${net[@]}" |
    node -e '
      const text = require("fs").readFileSync(0, "utf8");
      const field = (name) => (text.match(new RegExp(name + " = ([0-9_]+)")) ?? [])[1]?.replace(/_/g, "");
      if (!/Ok = record/.test(text) || !field("id")) { console.error(text); process.exit(1); }
      console.log(field("id"), field("executable_after_ns"), field("expires_at_ns"));
    '
}

# Runs the upgrade to release N, waits until it is installed, and checks
# what the wallet runs.
upgrade() {
  local n=$1 version=${versions[$1 - 1]} action=action_$1 status reply
  reply=$(icp canister call "$wallet" execute "(${!action} : nat64)" --candid candid/wallet.did "${net[@]}")
  case $reply in
    *"Ok = record"*) ;;
    *) echo "The wallet would not run the upgrade to $version:" >&2; echo "$reply" >&2; exit 1 ;;
  esac
  # The wallet sends the install again 1, 3, 7, 15 and 31 minutes after the
  # first send if the IC refused it, and calls it failed after an hour.
  for tick in $(seq 750); do
    status=$(icp canister call "$wallet" upgrade_status '()' --query --candid candid/wallet.did "${net[@]}")
    case $status in
      *Idle*) break ;;
      *Failed*) echo "The upgrade to $version failed:" >&2; echo "$status" >&2; exit 1 ;;
    esac
    [ "$tick" = 13 ] && echo "Still installing after a minute; the wallet sends the install again."
    sleep 5
  done
  case $status in
    *Idle*) ;;
    *) echo "The upgrade to $version is still installing after an hour:" >&2; echo "$status" >&2; exit 1 ;;
  esac
  check_running "$n"
}

# Checks, from what the IC tells anyone, that the wallet runs release N and
# is its own only controller.
check_running() {
  local version=${versions[$1 - 1]} hash=hash_$1 public named
  public=$(icp canister status "$wallet" --public --json "${net[@]}" |
    node -pe '
      const status = JSON.parse(require("fs").readFileSync(0, "utf8"));
      status.module_hash.replace(/^0x/, "") + " controlled by " + status.controllers.join(", ")')
  named=$(icp canister metadata "$wallet" b3wallet:release "${net[@]}")
  if [ "$public" != "${!hash} controlled by $wallet" ] || [ "$named" != "$version" ]; then
    echo "Wallet $wallet should run $version, ${!hash}, and be its own only controller." >&2
    echo "It runs $public, and names itself \"$named\"." >&2
    exit 1
  fi
}

# Checks that every page the wallet serves answers, through the HTTP gateway,
# which on mainnet verifies each response's certificate before passing it on.
check_pages() {
  local path code page
  page=$(page_of "$wallet")
  for path in / /index.html /claim /send /approvals /app.js /app.css; do
    code=$(curl -s -o /dev/null -w '%{http_code}' "$page$path")
    if [ "$code" != 200 ]; then
      echo "$page$path answers $code." >&2
      exit 1
    fi
  done
}

print_costs() {
  echo
  cat "$costs"
}

if [ "$half" = start ]; then
  if [ -e "$state/state.env" ]; then
    echo "The gate on $env has started already: run $0 finish -e $env, or remove $state to start over." >&2
    exit 1
  fi
  me=$(icp identity principal)
  if [ "$me" = 2vxsx-fae ]; then
    echo "The anonymous identity cannot pay for the gate. Make another one the default:" >&2
    echo "  icp identity new me && icp identity default me" >&2
    exit 1
  fi
  if [ "$env" = local ]; then
    icp network status >/dev/null 2>&1 || icp network start -d
  fi
  have=$(balance_of you)
  short=$(($(echo $needed | digits) - have))
  if [ "$short" -gt 0 ]; then
    echo "The gate needs about 7.1T cycles in $me's cycles ledger account, which holds $(cycles_text "$have" | tr -d +)." >&2
    echo "Buy the rest with ICP: icp cycles mint --cycles $(((short + 99999999999) / 100000000000))00b ${net[*]}" >&2
    exit 1
  fi

  echo "Building the wallet, release channel and factory"
  cargo build -q -p wallet -p release_channel -p factory --target wasm32-unknown-unknown --release --locked
  cargo build -q -p release-tool --locked
  mkdir -p "$state"
  signers=()
  for n in 1 2 3; do
    signers+=("$(ed25519 new "$state/signer-$n.pem")")
  done
  signer_blobs="blob \"${signers[0]}\"; blob \"${signers[1]}\"; blob \"${signers[2]}\""
  cat >"$costs" <<EOF
# The mainnet gate on $env

Started $(date -u '+%Y-%m-%d %H:%M UTC') by $me. Each row is what a step
changed each balance by, in cycles: - paid, + received. Reading the channel's
and the factory's balance for a row costs each of them about 1.9M cycles, and
every canister pays a little each second to hold its memory.

| Step | Took | You | Channel | Factory | Wallet |
| --- | --- | --- | --- | --- | --- |
EOF
  before_you=$have
  step_started=$(date +%s)

  echo "Creating the release channel"
  channel=$(icp canister create --detached -q --cycles "$channel_cycles" "${net[@]}")
  logged icp canister install "$channel" --wasm "$target/release_channel.wasm" "${net[@]}" --args "(variant { Init = record {
    publishers = vec { principal \"$me\" };
    release_signers = vec { $signer_blobs };
    release_threshold = 2 : nat8;
  } })"
  record "Create and install the release channel"

  echo "Publishing ${versions[0]}"
  publish 1
  record "Publish ${versions[0]} ($chunks chunks)"

  echo "Creating the factory"
  factory=$(icp canister create --detached -q --cycles "$factory_cycles" "${net[@]}")
  logged icp canister install "$factory" --wasm "$target/factory.wasm" "${net[@]}" --args "(variant { Init = record {
    release_channel = principal \"$channel\";
    release_signers = vec { $signer_blobs };
    release_threshold = 2 : nat8;
  } })"
  record "Create and install the factory"

  echo "Creating the wallet through the factory"
  # The ledger's fee for create_canister_from comes out of the allowance too.
  logged icp canister call "$cycles_ledger" icrc2_approve "(record {
    spender = record { owner = principal \"$factory\" };
    amount = $(($(echo $wallet_cycles | digits) + 100000000)) : nat;
  })" "${net[@]}"
  record "Let the factory take the wallet's cycles"
  expires=$(($(date +%s) + 2 * 60 * 60))000000000
  created=$(icp canister call "$factory" create_wallet "(record {
    claim_public_key = blob \"$(ed25519 new "$state/claim.pem")\";
    claim_expires_at_ns = $expires : nat64;
    version = opt \"${versions[0]}\";
    cycles = $wallet_cycles : nat;
    from_subaccount = null;
  })" --candid candid/factory.did "${net[@]}")
  wallet=$(echo "$created" | sed -n 's/.*wallet = principal "\([^"]*\)".*/\1/p')
  if [ -z "$wallet" ]; then
    echo "The factory did not create the wallet:" >&2
    echo "$created" >&2
    exit 1
  fi
  record "Create the wallet with ${versions[0]}"
  check_running 1

  echo "Claiming wallet $wallet"
  signature=$(ed25519 sign "$state/claim.pem" "claim:$wallet:$me")
  logged icp canister call "$wallet" claim "(record { signature = blob \"$signature\" })" \
    --candid candid/wallet.did "${net[@]}"
  record "Claim the wallet"
  check_pages

  echo "Publishing ${versions[1]} and ${versions[2]}"
  publish 2
  publish 3
  record "Publish ${versions[1]} and ${versions[2]} ($chunks chunks each)"

  echo "Proposing both upgrades"
  proposed=$(propose_upgrade 2)
  read -r action_2 runs_after_ns expires_at_ns <<<"$proposed"
  proposed=$(propose_upgrade 3)
  read -r action_3 _ _ <<<"$proposed"
  record "Propose both upgrades"

  runs_after=$(date_text "$runs_after_ns")
  expires_at=$(date_text "$expires_at_ns")
  print_costs
  cat <<DONE

Wallet $wallet runs ${versions[0]}, is its own only controller, and is yours.
Its pages answer at $(page_of "$wallet")/.

Its upgrades to ${versions[1]} and ${versions[2]}, actions $action_2 and $action_3, can run
from $runs_after until $expires_at. Then finish the gate with:

  $0 finish -e $env
DONE
  exit 0
fi

if [ ! -e "$state/state.env" ]; then
  echo "The gate on $env has not started: run $0 start -e $env first." >&2
  exit 1
fi
. "$state/state.env"
now_ns=$(date +%s)000000000
if [ "$now_ns" -lt "$runs_after_ns" ]; then
  echo "Too early: the upgrades can run after $(date_text "$runs_after_ns")." >&2
  exit 1
fi
if [ "$now_ns" -ge "$expires_at_ns" ]; then
  echo "Too late: the upgrade actions expired. Remove $state and start over." >&2
  exit 1
fi

record "Wait for the timelock"
for n in 2 3; do
  echo "Upgrading the wallet to ${versions[$n - 1]}"
  upgrade "$n"
  record "Upgrade to ${versions[$n - 1]}"
done
check_pages
print_costs
cat <<DONE

Wallet $wallet installed ${versions[1]} and then ${versions[2]} on itself, and
its pages still answer at $(page_of "$wallet")/. The channel $channel and
the factory $factory are yours to delete:

  icp canister stop $channel ${net[*]} && icp canister delete $channel ${net[*]}
  icp canister stop $factory ${net[*]} && icp canister delete $factory ${net[*]}
DONE
