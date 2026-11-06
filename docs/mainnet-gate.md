# The mainnet gate

Step 14 of [the v0 plan](plan-v0.md): deploy on mainnet, have a wallet
upgrade itself from v(n) to v(n+2) by chunked self-upgrade, and record what
each step costs in cycles. `scripts/mainnet-gate.sh` does it in two halves,
a week apart, because a wallet runs an upgrade only after its 7-day timelock.

The gate uses its own release channel, factory and wallet, and three signer
keys it makes for itself. The releases it publishes, `gate-1`, `gate-2` and
`gate-3`, are the wallet's own Wasm, each stamped with a public metadata
section, `b3wallet:release`, that names it. The gate's channel, factory and
wallet trust only the gate's keys, so nothing else ever installs these
releases, and the gate's wallet never installs a real one.

## Before you start

- icp-cli, ic-wasm and Node, the Rust toolchain of `rust-toolchain.toml`, and
  a clang that compiles to WebAssembly (on a Mac, Homebrew's LLVM), as for
  `scripts/try-local.sh`.
- An icp-cli identity other than the anonymous one as the default, holding
  about 7.1T cycles in its cycles ledger account:

  ```sh
  icp cycles balance -e ic
  icp cycles mint --cycles 7.1t -e ic   # converts ICP from the identity's account
  ```

  The channel and the factory get 1T each and the wallet the factory's
  minimum, 5T; creating a canister costs 0.5T of that.
- A checkout of `main`, run from its root. The gate keeps its state in
  `.icp/gate/ic/`: the canister ids, the gate's keys, the releases and the
  costs. Keep it until the second half has run.

## First half

```sh
./scripts/mainnet-gate.sh start
```

Or, to run on the SEV-SNP subnet `re2t4`, the one confidential subnet open
to anyone, with the guard loaded:

```sh
./scripts/mainnet-gate.sh start \
  --subnet re2t4-faa75-v3vhk-kdmdr-uyrkl-aik2l-ixd6u-p3fyr-zlfkc-6c5af-zae \
  --guard ../candid-profile/packages/profile-router/guard/.cache/guard-round11.bin
```

`--subnet` creates the channel and the factory there, and has the factory
create the wallet there (`create_wallet`'s `subnet`); the script checks
where the wallet landed. `--guard` takes only the file the wallet pins
([decision 0012](decisions/0012-guard-in-wallet.md)), checked before
anything is spent; after the claim it uploads it and loads it, and the
second half checks that each upgrade loaded it again.

It builds the wallet, the release channel and the factory, then:

1. creates and installs the release channel, with your identity as its
   publisher and the gate's three keys as its signers;
2. publishes `gate-1`;
3. creates and installs the factory;
4. approves the factory to take the wallet's cycles from your account, and
   creates the wallet with `gate-1` through the factory;
5. checks that the wallet runs `gate-1` and is its own only controller, from
   what the IC tells anyone;
6. claims the wallet with your identity, and checks that every page answers
   through the HTTP gateway, which verifies each page's certificate;
   with `--guard`, uploads the guard file and loads it;
7. publishes `gate-2`, tested from `gate-1`, and `gate-3`, tested from
   `gate-2`;
8. proposes both upgrades.

It takes a few minutes and prints the wallet's id and when the upgrades can
run: from 7 days after the proposals until 14 days after, when they expire.

## Second half

Between 7 and 14 days later:

```sh
./scripts/mainnet-gate.sh finish
```

It runs the upgrade to `gate-2`, waits for the wallet to report it done, and
checks again that the wallet runs `gate-2` and is its own only controller; then
the same for `gate-3`, and checks the pages once more. The script refuses to
run before the timelock ends or after the actions expired.

## The costs

Each half adds its rows to `.icp/gate/ic/costs.md` and prints the table: for
each step, how long it took and what it changed the cycle balance of your
account, the channel, the factory and the wallet by. Reading the channel's
and the factory's balance for a row costs each of them about 1.9M cycles, and
every canister pays a little each second to hold its memory, so the rows
between the big costs show those. The row "Wait for the timelock" is the week
of holding memory, charged when each canister next ran.

A rehearsal on icp-cli's local network, whose application subnet charges as a
13-node subnet on mainnet does, recorded on 2026-11-05 (main at `df725dc`):

| Step | Took | You | Channel | Factory | Wallet |
| --- | --- | --- | --- | --- | --- |
| Create and install the release channel | 2s | -1.000T | +495.142B | | |
| Publish gate-1 (7 chunks) | 5s | 0 | -14.579B | | |
| Create and install the factory | 2s | -1.000T | -1.944M | +493.700B | |
| Let the factory take the wallet's cycles | 0s | -100.000M | -1.944M | -1.944M | |
| Create the wallet with gate-1 | 7s | -5.000T | -492.853M | -14.277B | +4.485T |
| Claim the wallet | 0s | 0 | -2.334M | -1.944M | -13.437M |
| Publish gate-2 and gate-3 (7 chunks each) | 11s | 0 | -29.805B | -2.240M | -0.456M |
| Propose both upgrades | 1s | 0 | -1.944M | -1.944M | -35.394M |
| Wait for the timelock | 7d 2h | 0 | -18.300B | -10.661B | -24.539B |
| Upgrade to gate-2 | 7s | 0 | -497.007M | -2.188M | -29.580B |
| Upgrade to gate-3 | 7s | 0 | -529.312M | -2.153M | -29.578B |

With `--subnet` (the local application subnet) and `--guard`, on 2026-11-06
with round 9's file (round 11's, pinned since, is 20 bytes longer), the rows
that differ:

| Step | Took | You | Channel | Factory | Wallet |
| --- | --- | --- | --- | --- | --- |
| Upload and load the guard (24 chunks) | 9s | 0 | -1.944M | -2.153M | -95.582B |
| Wait for the timelock | 7d 2h | 0 | -18.300B | -10.662B | -50.941B |
| Upgrade to gate-2 | 7s | 0 | -496.537M | -1.944M | -35.815B |
| Upgrade to gate-3 | 7s | 0 | -529.469M | -2.275M | -35.919B |

Holding the guard's 44 MB in stable memory and again in the heap doubles
what the week of holding memory costs the wallet, and each upgrade reads the
file again for about 6B more. The mainnet numbers replace these once the gate
has run.

## What an upgrade needs

The IC holds back what an install's instruction limit would cost, about 0.3T
cycles on a 13-node subnet, and refuses an install the canister cannot pay for
above what it keeps to stay running (its freezing threshold: 30 days of
holding its memory). The upgrade, staging included, then costs the wallet
about 30B for a release of 7 chunks, 36B with the guard loaded, and the rest
comes back. A wallet that cannot spend 0.35T, plus 4B
a chunk to stage the release, refuses to execute the upgrade with
`InsufficientCycles`, which says what it can spend and what it needs; the
action stays ready for after a top-up:

```sh
icp canister top-up --amount 0.2t <wallet id> -e ic
```

The IC also refuses an install for a few seconds after another one on the same
canister. A wallet installs itself one-way and never hears the refusal, so it
sends the install again 1, 3, 7, 15 and 31 minutes after the first send while
the old version still runs, and marks the upgrade `Failed` after an hour.

## Afterwards

The channel and the factory are yours to delete; the script prints the
commands. `icp canister delete` returns a canister's cycles to your account
first. The wallet stays, with the cycles it has left, about 4.4T: it is its
own only controller, so nobody can delete it. It is a working wallet you own,
which trusts only the gate's keys for releases.

## Rehearsing it locally

`-e local` runs both halves on icp-cli's local network, which the first half
starts. The second half needs the local network's clock and icp-cli's 7 days
on; on Linux, with `faketime`:

```sh
./scripts/mainnet-gate.sh start -e local
port=$(node -pe 'require("./.icp/cache/networks/local/descriptor.json")["pocketic-config-port"]')
target=$(node -pe "String(BigInt(Date.now()) * 1000000n + 170n * 3600n * 1000000000n)")
# PocketIC answers 409 while it is busy: repeat until get_time shows the jump.
curl -s -X POST -H 'Content-Type: application/json' \
  -d "{\"nanos_since_epoch\": $target}" "http://127.0.0.1:$port/instances/0/update/set_time"
curl -s "http://127.0.0.1:$port/instances/0/read/get_time"
sleep 20   # the network charges the week of holding memory at its next charge
faketime -f +170h ./scripts/mainnet-gate.sh finish -e local
```

The local HTTP gateway checks certificates against its own clock, which did
not move, so the second half's page check fails with 503; the pages answer at
`http://<wallet id>.raw.localhost:8000/`, and the PocketIC tests check them
certified after an upgrade. Stop the network afterwards and remove
`.icp/cache/networks/local`, since its clock stays a week ahead.
