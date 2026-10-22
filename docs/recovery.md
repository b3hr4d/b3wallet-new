# Recovering a wallet with its recovery controller

[Decision 0009](decisions/0009-recovery-controller.md) gives each wallet a
small controller canister that only the owners' recovery keys can drive. It
can repair the wallet in place when the wallet's pages, its sign-in, its API
or its own upgrade method break, without anything from the wallet. This
guide is for owners: how to make the keys, create and check a controller,
hand a wallet to it, and then upgrade, repair, rotate keys or replace the
controller with `recovery-tool` and icp-cli alone.

This has had no external security review. Keep only small amounts in a
wallet until it has, and rehearse every step below on a local network
before you rely on it.

## What you need

- `recovery-tool`, built from a checkout you trust:
  `cargo build -p recovery-tool --release --locked` writes
  `target/release/recovery-tool`. It needs no network.
- [icp-cli](https://github.com/dfinity/icp-cli) 1.7.0, which this guide and
  the tests use: `npm install -g @icp-sdk/icp-cli@1.7.0`. Set
  `DO_NOT_TRACK=1` to turn off its telemetry.
- `openssl`, to make keys.
- Cycles in your icp-cli identity's cycles ledger account, about 3.5T to
  create and fund a controller (`icp cycles balance -n ic`).

Run icp-cli outside any icp-cli project, such as this repository: it looks
for an `icp.yaml` upwards and would use that project's network.
`-n ic` names the IC mainnet. The commands below write their files to the
current directory and use these names:

```sh
export DO_NOT_TRACK=1
WALLET=<the wallet's canister id>
DID=<path to this repository>/candid
```

## 1. Make the recovery keys

The default policy has three keys: one you use day to day, one on a
hardware-backed or offline computer, and one on paper. Make each on the
computer that will hold it:

```sh
openssl genpkey -algorithm ed25519 -out daily.pem
recovery-tool key --key daily.pem
```

```
Public key: f5e65c3677eb1b786d62099eb8d4f74c515e9b28fff3e5d485e3db995d7eb62a
Principal:  3epej-fsbkw-pnhag-bdosf-5bdkr-fz5ai-5dudm-2zbby-ar47g-xcei2-aqe
```

The public key goes into the policy; the principal is the identity the key
calls the controller as. Give each key to icp-cli too:

```sh
icp identity import recovery-daily --from-pem daily.pem
```

icp-cli keeps it in the system keyring; on a computer without one, add
`--storage password`. For the paper key, print `paper.pem`, check the print
by typing it back in on a clean computer and running `recovery-tool key` on
it, then delete the file.

A recovery key is not an Internet Identity and does not depend on any page
of the wallet. Each key is one authority: two keys on one laptop are one
authority, however many there are. A policy needs at least three keys, so
that one stolen key cannot act alone and one lost key cannot block the
rest. Keep at least the sovereign threshold of keys (2 by default) where one
theft, fire or lost device cannot take them together, and labels that do not
say where a key is kept: the policy is public.

## 2. Build the controller and know its hash

```sh
scripts/build-controller.sh
```

```
8455334b712142130bf3a405a75b2834bc615ab692f23c88e5983e057971b9e3  .../target/controller-build/wasm32-unknown-unknown/release/controller.wasm
```

It builds with the toolchain `rust-toolchain.toml` pins and maps the
checkout's and Cargo's paths away, so the same commit gives the same bytes in
any directory; CI builds it in two and compares. Builds on other operating
systems have not been compared yet. Compare the hash with one someone else
built from the same commit before you trust a controller with it.

## 3. Hand a wallet to a controller

A wallet that controls itself gives control away only through its own
`HandOverControl` action. Before it, the controller has no power over the
wallet; after it, the controller is the wallet's only controller and the
wallet's own upgrade action stops working ("the wallet is not its own
controller"): upgrades go through the recovery keys (section 4).

### 3.1 Create the controller

```sh
WALLET_SUBNET=<the wallet's subnet>
CONTROLLER=$(icp canister create --detached --cycles 3t --subnet $WALLET_SUBNET -q -n ic)
```

The controller must run on the wallet's subnet, so that repairing the wallet
depends on no other subnet. The IC dashboard shows a canister's subnet at
`https://dashboard.internetcomputer.org/canister/<id>`; the wallet checks
it again itself before it hands itself over, so a wrong value there cannot
hand the wallet to a controller elsewhere, only waste this one.

Leave every other setting at the IC's default. Nobody can change the
controller's settings once its controllers are gone, so the wallet refuses a
controller whose settings could leave it frozen or unable to grow: a
freezing threshold outside 30 to 365 days, a memory or compute allocation, a
reserved cycles limit below 5T or a Wasm memory limit below 1 GiB.

### 3.2 Write the policy

```sh
recovery-tool policy \
  --authority <daily public key>=daily \
  --authority <hardware public key>=hardware \
  --authority <paper public key>=paper \
  --normal-threshold 1 --sovereign-threshold 2 > policy.txt
```

It writes the policy to `policy.txt` and tells you what it means:

```
  a signed release needs 1 of 3 keys and 2 days; anything else needs 2 of 3 keys and 7 days
  no release signers: every install takes the sovereign route
  policy SHA-256 fa7aaef13ec231dbf612225c3c3a113effc0d8d11a6652739a86a2aed44ae9db
```

`--normal-delay` and `--sovereign-delay` change the delays (at least 24 hours
and 72 hours, at most 30 days). `--release-signer KEY`, repeated, and
`--release-threshold N` let a release those signers signed take the shorter
route; leave them out to check every module yourself.

The tool and the controller refuse a policy that breaks these rules:

- 3 to 10 keys, each with its own label of 1 to 64 printable ASCII
  characters (labels may not differ only in case, nor start or end with a
  space).
- A sovereign threshold of at least 2 and at most one less than the number
  of keys, so one stolen key cannot act alone and one lost key never blocks
  a key rotation.
- A normal threshold of at least 1 and below the sovereign threshold, and a
  sovereign delay longer than the normal delay.
- Release signers that are not recovery keys: someone who signs releases
  must not also be able to approve them.

### 3.3 Prove each key, and write the install argument

On each key's computer:

```sh
recovery-tool proof --key daily.pem --controller $CONTROLLER --wallet $WALLET > proof-daily.txt
```

A proof is a signature that names this controller, this wallet and the key,
so a typo in a public key, or a key nobody holds, fails the install. Then:

```sh
recovery-tool init --controller $CONTROLLER --wallet $WALLET --policy policy.txt \
  --proof proof-daily.txt --proof proof-hardware.txt --proof proof-paper.txt > init.txt
```

### 3.4 Install it, then remove its controllers

```sh
icp canister install $CONTROLLER --mode install --wasm controller.wasm --args-file init.txt -n ic
icp canister call $CONTROLLER health '()' --candid $DID/controller.did --identity recovery-daily -n ic
icp canister settings update $CONTROLLER --remove-all-controllers --force -n ic
```

Before you remove its controllers, check that `health` names the wallet's
subnet: while you still control the controller you can delete it and start
again. `health` is a call from one of the keys, since the controller takes
messages from outside the IC only from its keys.

Install it exactly once, and do not reinstall, upgrade, uninstall or load a
snapshot into it: the wallet refuses a controller whose history shows any of
those, since they could leave it holding state its own install did not
write. If anything went wrong before you removed its controllers, create a
new controller instead. Once its controllers are gone nobody can change it;
an unused one only costs its cycles.

### 3.5 Check it

```sh
icp canister status $CONTROLLER --public --json -n ic > controller-status.json
icp canister status $WALLET --public --json -n ic > wallet-status.json
icp canister call $CONTROLLER state '()' --candid $DID/controller.did \
  --identity recovery-daily --output hex -n ic > state.hex
icp canister call $CONTROLLER health '()' --candid $DID/controller.did \
  --identity recovery-daily --output hex -n ic > health.hex
recovery-tool check --controller $CONTROLLER --wallet $WALLET \
  --module-hash <the hash from step 2> --policy policy.txt \
  --controller-status controller-status.json --wallet-status wallet-status.json \
  --state state.hex --health health.hex --wallet-subnet $WALLET_SUBNET \
  --before-hand-over
```

From the rehearsal on icp-cli's local network:

```
Controller ibrml-ld777-77774-qaabq-cai: module 8455334b712142130bf3a405a75b2834bc615ab692f23c88e5983e057971b9e3, controllers [].
Wallet igqk7-g3777-77774-qaaba-cai: module 96147e6f36dbd8d7ccedfabfcaa5aa93f3e5ff71f01e86fba11ce1093c6df7da, controllers [igqk7-g3777-77774-qaaba-cai].
The controller runs on subnet srkif-m33vd-rsyll-hrrsp-o6jlq-a6ad4-7pyvj-hi6b5-ncfgy-qdn5s-tqe, with a freezing threshold of 30 days.
The controller's state: policy version 1, next nonce 1, 2_486_816_167_269 cycles.
Everything matches.
Propose the hand-over on the wallet's Recovery card with:
  Controller id     ibrml-ld777-77774-qaabq-cai
  Its module hash   8455334b712142130bf3a405a75b2834bc615ab692f23c88e5983e057971b9e3
  Its policy hash   fa7aaef13ec231dbf612225c3c3a113effc0d8d11a6652739a86a2aed44ae9db
```

`canister status --public` reads the module hash and controllers from the
IC's certified state, so no canister can lie about them. The state and
health come from update calls, which go through consensus; health is what
the controller's own code reads from the IC about itself, so it is as good
as the module hash you checked. With `--before-hand-over`, `check` also
refuses a controller that has taken an operation or changed its policy: the
wallet hands itself only to a controller as its install left it. Leave out
`--wallet-subnet` and `check` checks everything but the subnet. Any mismatch
is printed, and the tool exits with 1.

### 3.6 Hand the wallet over

On the wallet's start page, an owner opens the **Recovery controller** card,
enters the three values `check` printed, and proposes the hand-over. It is a
policy change for the wallet: it needs the wallet's `SetPolicy` quorum and
waits for its timelock (7 days by default) on the Approvals page. Then an
owner or approver runs it there.

When it runs, the wallet checks again, itself: the controller runs the named
module, has no controller, its history is one install of that module (from
the IC's `canister_info`, which only canisters can read), it serves this
wallet with the named policy, has not handed it on and has taken no
operation, and its `health` shows the wallet's own subnet and settings that
cannot strand it. Only then does it set its controllers to exactly the
controller, in one call that also drops itself.

### 3.7 Check again

Run step 3.5's commands without `--before-hand-over`. The wallet's
controllers must now be exactly the controller.

### If the hand-over stops part way

| Stopped at | What holds | What to do |
| --- | --- | --- |
| Keys made, nothing created | Nothing changed | Nothing |
| Controller created or installed, its controllers not yet removed | You control the controller; it has no power over the wallet | Create a new controller rather than reinstalling this one |
| Controllers removed, `check` fails | The controller is unusable for this wallet; the wallet is untouched | Create a new controller; the failed one keeps its cycles |
| Hand-over proposed, not executed | Nothing changed | Let it run, or let it expire |
| A check fails when the action runs | Nothing changed; the action ends `Failed` with the reason | Fix the cause, propose again |
| The call that sets the controllers fails | Nothing changed: the call is atomic; the action stays ready | Execute it again |
| The call succeeded but its answer was lost | The controller controls the wallet; the action is not marked | Execute it again: it sees the controllers and ends `Executed` |
| Done | The controller is the wallet's only controller | Section 4 |

At no point does the wallet have a controller other than itself or the
controller, and nothing in the hand-over leaves anyone else, its developers
included, with authority over it.

## 4. Drive the controller

Read its state first; operations name its policy version and next nonce:

```sh
icp canister call $CONTROLLER state '()' --candid $DID/controller.did --identity recovery-daily -n ic
icp canister call $CONTROLLER operations '(record { before = null; limit = 10 : nat8 })' \
  --candid $DID/controller.did --identity anonymous --query -n ic
```

The controller pays for every message it takes, so it takes messages from
outside the IC only from its recovery keys; icp-cli says `Canister rejected
the message` to anyone else. Anyone can still read it with `--query`, which
one node answers, and a key's call without `--query`, as for `state` above,
gets an answer that went through consensus. Only a recovery key can change
the controller's state.

Every operation names the controller, the wallet, the policy version, a
nonce, an expiry and what it does. Keys sign its digest, so a signed
operation cannot be replayed, changed or used on another controller or
wallet. The expiry must fall after the operation's delay ends and at most 60
days ahead. Each key may propose at most 8 operations in 24 hours, and hold
at most 4 open at once, so a stolen key cannot fill the controller.

### 4.1 Upgrade or repair the wallet

Write the operation, with the Wasm and the install argument the release's
notes name (`(variant { Upgrade })` for this version of the wallet):

```sh
recovery-tool install --controller $CONTROLLER --wallet $WALLET \
  --policy-version 1 --nonce 1 --expires-in 14d --wasm wallet.wasm \
  --arg-candid '(variant { Upgrade })' --interface $DID/wallet.did > upgrade.did
```

Read it before anyone signs it, on each key's computer:

```sh
recovery-tool show --interface $DID/wallet.did upgrade.did
```

```
Operation 1 for recovery controller ibrml-ld777-77774-qaabq-cai of wallet igqk7-g3777-77774-qaaba-cai,
under policy version 1, expiring at 2026-11-13 20:25:40 UTC.
It would upgrade the wallet's code, keeping its state
to the module with SHA-256 96147e6f36dbd8d7ccedfabfcaa5aa93f3e5ff71f01e86fba11ce1093c6df7da, in 5 chunks,
with an install argument of 66 bytes, SHA-256 024a270aebd3f0778ac4bcf8a55f992b35d4a78d45baeccab7280735a11b4035.
Route: sovereign. No release signer vouches for this module: check it yourself.
Digest: 082277c4a5cec93fa237aad83da68f6c88373eabaf30048a37716e90abfd5b56
Install argument: (variant { Upgrade })
No approvals in this file yet.
```

Then each key that approves it signs the file, which you pass from computer
to computer:

```sh
recovery-tool sign --key daily.pem upgrade.did
recovery-tool sign --key hardware.pem upgrade.did
```

Any key proposes it, with the approvals in the file:

```sh
icp canister call $CONTROLLER propose --args-file upgrade.did \
  --candid $DID/controller.did --identity recovery-daily -n ic
```

A key that signs later signs the same file and sends its approval with
`recovery-tool approval upgrade.did > approval.did` and `icp canister call
$CONTROLLER approve --args-file approval.did …`. The file may hold approvals
the controller has already; each key counts once. Once the threshold is
met, upload the module:

```sh
recovery-tool chunks --wasm wallet.wasm --out chunks upgrade.did
for chunk in chunks/chunk-*.bin; do
  icp canister call $CONTROLLER upload_chunk --args-file $chunk --args-format bin \
    --candid $DID/controller.did --identity recovery-daily -n ic
done
```

Each answer lists the chunks still missing. After the delay, any key
executes it:

```sh
icp canister call $CONTROLLER execute '(1 : nat64)' --candid $DID/controller.did \
  --identity recovery-paper -n ic
```

For an upgrade, the controller stops the wallet, which waits until every call
the wallet made is answered, and empties the wallet's chunk store. If the
wallet cannot pay for the install, the controller gives it the cycles it
lacks. It then stages the chunks in the wallet's chunk store, installs from
them, and starts the wallet again, also when a step failed. A failed
execution leaves the operation ready until it expires, so you can fix the
cause and execute again without a new delay.

Choices:

- `--release manifest.did` names a release manifest from the release
  channel. If the policy trusts its signers, the operation takes the release
  route (the normal threshold and delay), and the controller checks the
  signatures, that the manifest names this module and chunks, and that it
  was tested to upgrade from the module the wallet runs now. Otherwise
  leave it out and take the sovereign route.
- `--skip-pre-upgrade`, when the running code's `pre_upgrade` traps:
  sovereign route.
- `--reinstall` erases the wallet's state. Use it last, when the code
  traps in `pre_upgrade` and `post_upgrade` alike or never lets the wallet
  stop (an upgrade then fails with `the wallet did not stop`). The
  controller uninstalls the code instead of stopping it, which ends every
  call the code was waiting on, and installs the new code fresh. The funds
  stay: they belong to the wallet's principal and to keys derived from its
  canister id, which a reinstall keeps. Its members, open actions, history
  and account list are gone, and the install argument must be an `Init`
  with a new claim key:

  ```sh
  openssl genpkey -algorithm ed25519 -out claim.pem
  recovery-tool key --key claim.pem   # its public key goes in the argument
  # Candid writes a blob's bytes as \xx: f5e6… becomes \f5\e6…
  blob() { echo "$1" | sed 's/../\\&/g'; }
  recovery-tool install … --reinstall --wasm wallet.wasm --interface $DID/wallet.did \
    --arg-candid '(variant { Init = record {
      claim_public_key = blob "<blob of the claim public key>";
      claim_expires_at_ns = <now + 30 days, in ns> : nat64;
      release_channel = principal "<the release channel>";
      release_signers = vec { blob "<a release signer>"; … };
      release_threshold = <N> : nat8;
      factory = null } })' > reinstall.did
  ```

  After it runs, an owner claims the wallet again from its claim link
  (`https://<wallet>.icp0.io/claim#<claim key>`, see the README) and adds
  the accounts again in the order they had: account n of a kind is
  derived from n, so the same accounts come back with their balances.

### 4.2 Rotate keys or change the policy

Write the new policy as in 3.2, prove each new key for this controller and
wallet as in 3.3, then:

```sh
recovery-tool set-policy --controller $CONTROLLER --wallet $WALLET \
  --policy-version 1 --nonce 2 --expires-in 14d --policy new-policy.txt \
  --proof proof-new.txt > rotate.did
```

Sign, propose and execute it as in 4.1 (no chunks). It needs the sovereign
threshold and delay. Executing it raises the policy version, which turns
every other open operation stale.

- A lost key: the remaining keys rotate it out, if they reach the sovereign
  threshold.
- A stolen key: cancel what it proposes (4.4), and rotate it out. It can
  cancel any open operation except a policy change that removes it, and it
  can take the nonce you signed for by proposing first, so you must write
  and sign again with the next one, but no more than 8 times a day. A
  policy change that removes it runs one sovereign delay after it is
  approved.

### 4.3 Move to a new controller version

The controller can never be upgraded. A new version takes over instead:
create and install a new controller for the same wallet (3.1 to 3.4, with
proofs for the new controller), check it with `--before-hand-over` left out
and expect only the wallet's controllers to differ, then:

```sh
recovery-tool replace --controller $CONTROLLER --wallet $WALLET \
  --policy-version 1 --nonce 3 --expires-in 14d \
  --new-controller $NEW --module-hash <its hash> --policy new-policy.txt > replace.did
```

Create the new controller on the wallet's subnet with the default settings,
as in 3.1, and propose nothing to it before the replacement runs: the old
controller hands the wallet only to a controller that has taken no
operation. Sign, propose and execute it as in 4.1. The old controller checks
the new one as the wallet did in 3.6, then sets the wallet's controllers to
exactly the new one, and acts no more.

### 4.4 Cancel an operation

Any key cancels an open operation:

```sh
recovery-tool cancel --key daily.pem unwanted.did > cancel.did
icp canister call $CONTROLLER cancel --args-file cancel.did \
  --candid $DID/controller.did --identity recovery-daily -n ic
```

### 4.5 Sign on a computer that never goes online

`recovery-tool` needs no network. icp-cli signs a call with no network too,
and another computer sends it:

```sh
# offline
icp canister call $CONTROLLER approve --args-file approval.did \
  --candid $DID/controller.did --identity recovery-paper -n ic --sign-only signed.json
# online, within the five minutes it prints
icp message send signed.json --yes
```

`--valid-from 55m` places the five-minute window later, for the time it
takes to carry the file across.

### 4.6 Start the wallet

If an execution left the wallet stopped, the controller's state says
`wallet_stopped = true`, and it tries to start it every minute. Any key can
also ask:

```sh
icp canister call $CONTROLLER start_wallet '()' --candid $DID/controller.did \
  --identity recovery-daily -n ic
```

## 5. Cycles

Anyone can top up either canister, with no key and no controller:

```sh
icp canister top-up --amount 1t $CONTROLLER -n ic
```

An install needs the wallet to be able to spend 0.35T above its freezing
reserve, and the controller 100B plus 2B per chunk for its own calls. When
the wallet has less, the controller gives it what it lacks, so code that
burned the wallet's cycles can still be repaired: the controller then needs
its own share plus the wallet's shortfall. If it has less, it refuses with
`InsufficientCycles`, naming the canister and both amounts, before it stops
anything; top up either canister and execute again. Measured on icp-cli's
local network, which charges as a 13-node subnet does: upgrading a 4.9 MB
wallet (5 chunks), from proposal to execution, cost the controller 33B
cycles. Keep a few trillion cycles on the controller, and watch it: a frozen
controller cannot repair anything until someone tops it up.

## 6. When something says no

| Message | Meaning |
| --- | --- |
| `only a recovery key of this controller can do this` | The identity is not one of the policy's keys |
| icp-cli: `Canister rejected the message` | The identity is not one of the policy's keys; read with `--query`, or call as a key |
| `… must expire after its delay of N hours ends` | Write it again with a later `--expires-in` |
| `… in the last 24 hours` | This key proposed 8 operations in the last day; wait, or propose with another key |
| `this key proposed 4 operations that are open` | Let them run, expire or be cancelled |
| `the wallet did not stop: …` | The wallet's code holds calls open; repair it with `--reinstall` (4.1) |
| `… not the wallet's subnet …` | Create the controller on the wallet's subnet (3.1) |
| `… changed its policy or taken operations already` | Create a new controller and hand the wallet to it before using it |
| `… freezing threshold …`, `… allocation …`, `… reserved cycles limit …`, `… Wasm memory limit …` | The controller was created with settings that could strand the wallet; create a new one with the defaults |
| `the operation's nonce is N, and the controller's next nonce is M` | Write it again with nonce M |
| `the operation is for policy version N, and the policy is at version M` | The policy changed; write it again |
| `QuorumNotMet` | More keys must approve |
| `TimelockActive { ready_at_ns }` | Wait until then |
| `ChunksMissing` | Upload the chunks it lists |
| `Closed = variant { Stale }` | The policy changed, or the wallet was handed on, after it was proposed |
| `Closed = variant { Expired }` | Propose it again |
| `release … was not tested to upgrade from the module the wallet runs` | Take the sovereign route |
| `InsufficientCycles { canister, available, needed }` | Top up that canister |
| `ExecutionInProgress` | Another execution runs; wait |
| `a step took longer than an hour …` | The controller gave up on a stuck execution; execute again |
| icp-cli: `Invalid request expiry` | The computer's clock is wrong |

## 7. What the checks cannot see

- `check` cannot read a canister's history: the IC answers `canister_info`
  only to canisters. The wallet reads it before the hand-over and the old
  controller before a replacement; afterwards the controller has no
  controller, so its history cannot grow.
- Nobody but a controller can read a canister's settings from outside, so
  the settings and subnet `check` and the wallet see come from the
  controller's own `health`, which asks the IC about itself. That answer is
  as trustworthy as the module the controller runs, which `check` compares
  with your build. Such a setting can only stop the controller from working,
  never give anyone power over it.
- The controller's module hash is only as good as the build you compare it
  with (section 2).
- Neither `check` nor the wallet shows you operations proposed to the
  controller later. Read `operations` from time to time: a key you lost
  track of could propose something, and only its delay gives you time to
  cancel it.

## 8. The digest, for another client

A client in another language can compute what the keys sign from this
section alone. Every encoding starts with an ASCII domain and a zero byte.
Integers are big-endian; a byte string is its length as 4 bytes followed by
its bytes; a principal is the byte string of its raw bytes; a list is its
length as 4 bytes followed by its items.

**Policy** (`b3wallet-controller/policy`): the number of keys, then each
key's public key and label (UTF-8) as byte strings; the normal and sovereign
thresholds, one byte each; the normal and sovereign delays in nanoseconds, 8
bytes each; the number of release signers and each signer's public key as a
byte string; the release threshold, one byte. The policy hash is the SHA-256
of this.

**Operation** (`b3wallet-controller/operation`): the format byte 1; the
controller and the wallet; the policy version, the nonce and the expiry in
nanoseconds since the Unix epoch, 8 bytes each; then one of:

- `InstallCode`: the byte 1; the mode, as 1 then 1 or 0 for
  `skip_pre_upgrade`, or 2 for a reinstall; the Wasm's SHA-256; the number of
  chunks and each chunk's SHA-256 (chunks are 1 MiB, the last shorter); the
  SHA-256 of the install argument's bytes; then 0 for the sovereign route, or
  1 for the release route followed by the release's version (UTF-8), its Wasm
  size (8 bytes), the number of `tested_from` hashes and each hash, its notes
  URL (UTF-8) and its publication time (8 bytes). The manifest's module and
  chunk hashes are the install's own, and its signatures are not part of
  what the keys sign.
- `SetPolicy`: the byte 2, then the policy as above without its domain.
- `ReplaceController`: the byte 3, the new controller, its module hash and
  its policy hash.

The digest is the SHA-256 of the operation's encoding. A key approves an
operation with an Ed25519 signature over `b3wallet-controller/approve`, a
zero byte and the 32-byte digest, and cancels it with one over
`b3wallet-controller/cancel`, a zero byte and the digest. A proof of
possession is a signature over `b3wallet-controller/enroll`, a zero byte,
the controller, the wallet and the key's public key. A key calls as the
self-authenticating principal of its public key in DER
(`302a300506032b6570032100` followed by the 32 bytes).

### Test vectors

These are the digests `crates/recovery`'s `test_vectors` test pins. The
inputs:

- Keys from 32-byte seeds, every byte the seed: seed 1 is
  `8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c`, 2 is
  `8139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394`, 3 is
  `ed4928c628d1c2c6eae90338905995612959273a5c63f93636c14614ac8737d1`, 21 is
  `d54207da194977dcf46adbfec2bc2e75b52d5a8a42184fedfdc00024f0e3e8da`, 22 is
  `511c34a1a2cb521df16bb246b8de8e7997ce235c7e76b22a3d7503a24819dd8a`, 23 is
  `31debe55d37c722768b137131caa6087080b2e0b60b94bd785d14575cfa498bc`.
- The policy: keys 1, 2 and 3 labelled `key 1`, `key 2`, `key 3`; thresholds
  1 and 2; delays 48 hours and 7 days; release signers 21, 22 and 23;
  release threshold 2.
- The controller's raw bytes are `00000000000000010101`, the wallet's
  `00000000000000020101`. T0 is 1000 days in nanoseconds,
  `86400000000000000`.
- The install: policy version 1, nonce 0, expiry T0 + 10 days, an upgrade
  without `skip_pre_upgrade`, of a Wasm of 1 MiB + 10 bytes whose byte *i* is
  *i* mod 256 (two chunks), with the argument `DIDL\x00\x00`, on the
  sovereign route.
- The release: the same install on the release route, version `1.2.0`,
  `tested_from` one hash of 32 bytes of 5, notes URL
  `https://example.org/1.2.0`, published at T0.
- The policy change: the install's fields with nonce 1, to the policy with
  a sovereign delay of 14 days.
- The replacement: the install's fields with nonce 2, to controller
  `00000000000000030101`, module hash 32 bytes of 1, and the policy's hash.

| Vector | SHA-256 |
| --- | --- |
| policy | `f386a4d210393f76bdf24ea9f4303327f6896ffaf3bd1477b4b0902cf62d1fe1` |
| install | `3b7cc80f258157d2c400aa54f3715a326f4c9d2f5ee61344e0213e29f3c3250c` |
| release | `a1243672325ea5f24965f7d9301d48c5cc3ef084613d2a4e0362126aea40b584` |
| policy change | `905ed5a04f6fc07c386f7c422e6d7ca7ffa73d0c28e27bad6208880dd6799627` |
| replacement | `631db29dc67bd3a3c4259f56b6f538c25a5919346bee4fd21accb4437a8aaa96` |

`tools/recovery-tool/vectors.py` computes them again in Python from this
section, with no code from this repository; CI runs it.
