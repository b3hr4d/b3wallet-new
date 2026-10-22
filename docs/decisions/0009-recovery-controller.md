# 0009: A recovery controller, held by owner keys, controls each wallet

Status: proposed, 2026-10-22. Supersedes
[decision 0005](0005-rescue-path.md). Behrad asked for an owner-controlled
upgrade and recovery path that still works when the wallet's pages, its
authorization or its upgrade methods are broken, and chose the default
thresholds and delays below (three keys: one key and 48 hours for a signed
release, two keys and 7 days for everything else).

**Decision: each wallet gets its own small controller canister, which is the
wallet's only controller and has no controller itself. The controller acts
only on operations signed by Ed25519 recovery keys the owners hold, which it
stores itself. Every operation names the controller, the wallet, the code
and argument hashes, the install mode, an expiry, a nonce and the policy
version it was approved under, waits for its threshold and timelock, and
any one key can cancel it. A release our signers signed needs fewer keys and
a shorter delay than a repair they did not sign; nothing installs without
owner keys.**

## Why 0005 changes

0005 kept the wallet its own only controller, so nobody but its owners could
change its code. It also meant that a release which breaks the wallet's own
upgrade method, its authorization or its pages strands the wallet on that
release, and that the owners' authority lives only in the code it would need
to repair. The new controller keeps the first property and drops the second:
the owners' authority moves out of the wallet into a canister that the
wallet's code cannot change.

## What Orbit does, and what we take from it

We read Orbit's source at `dfinity/orbit@c766e61` (2026-10-19), not only its
docs. Orbit is alpha. Its station (the wallet) is controlled by an upgrader
canister, and the upgrader by the station:

| Orbit fact (file) | Consequence for us |
| --- | --- |
| The station creates its upgrader with controllers `[station, fallback]` and sets its own controllers to `[upgrader, fallback]` (`core/station/impl/src/services/system.rs`) | The wallet's code controls its rescuer: a malicious station release can upgrade the upgrader or change its controllers |
| The control panel deploys every station with `fallback_controller = NNS root` (`core/control-panel/impl/src/services/deploy.rs`) | NNS governance can replace both canisters' code; a third party holds authority |
| The disaster-recovery committee is set by `set_disaster_recovery_committee`, which only checks that the caller is a controller of the upgrader, that is the station (`core/upgrader/impl/src/controllers/disaster_recovery.rs`) | The broken or malicious code being recovered from decides who may recover it |
| Normal upgrades are authorized by `caller == station` (`WithAuthorization` in `core/upgrader/impl/src/upgrade.rs`) | Authorization because a request came from the wallet, which we must not do |
| A recovery request stores the whole module per member, runs once `quorum` members sent the same module hash and argument, with no timelock, no expiry beyond a week, no nonce and no policy version (`services/disaster_recovery.rs`) | No delay to notice and cancel, and requests survive committee changes until they are pruned |
| Committee members are station users' principals, normally Internet Identity principals for the Orbit app's origin | Recovery depends on the same login as daily use |
| Recovery can restore a canister snapshot by quorum | A restore reverts local state while transfers already sent stay sent |
| The disaster recovery user page is a draft that says to contact Orbit (`docs/src/content/docs/users/disaster-recovery.md`) | No documented owner-run recovery client |

We reuse no Orbit code: the upgrader depends on the station's API types, its
users and its account model, and its trust model is the one we are moving
away from. We take three ideas from it: one helper canister per wallet,
recovery requests that run only when enough members ask for the same module
hash and argument, and stop, install, start around every install, starting
the wallet again whatever fails.

## Authority over each canister

| Canister | Its controllers | Who can change its code | Who decides what it does |
| --- | --- | --- | --- |
| Wallet | The controller, and nothing else | The controller, on an operation the recovery keys approved | Its members and policy, as before, for everything but its code |
| Controller | None | Nobody; a new version replaces it through the owners (below) | The recovery keys, by signed operations |
| Factory, release channel, release signers | Unchanged | Their own controllers | They can withhold a release and sign one; neither installs anything |

The wallet is not one of its own controllers, so no wallet code can change
its controllers, install code, or reach the controller's privileged paths:
the controller gives a call from the wallet no more weight than a call from
anyone else.

## Recovery keys

A recovery key is an Ed25519 key pair the owner holds outside the wallet: a
key file the recovery tool makes on a separate computer, one kept offline,
or one printed and stored. The controller stores the public keys, at most
10, with a label each. A key signs operations with a domain-separated
message the recovery tool shows before it signs, and the same key is also an
IC identity: its self-authenticating principal is the only caller the
controller accepts for a call that changes its state, and the only sender
whose messages from outside the IC it takes at all, for any method, so
strangers cannot spend its cycles on messages it pays for. Its policy and
operations are public: anyone reads them with a query call, and a canister
with a call it pays for, so anyone can check who controls a wallet. A key's
label, printable ASCII and unlike every other label, should not say where
the key is kept.

- **Independent of the wallet's login.** Internet Identity gives a principal
  per origin, and a broken wallet origin, or a compromised anchor, would
  take every principal behind that anchor with it. A recovery key is not an
  Internet Identity principal and does not depend on any page. Each key is
  one authority; two keys held on the same device are not two authorities,
  and the guide says so.
- **Enrollment.** The controller is installed with its policy and a proof of
  possession for every key: a signature by the key over the controller's
  and the wallet's ids and the key itself. A typo, or someone else's key,
  fails at install.
- **Verification.** `recovery-tool check` takes the module hashes and
  controllers of both canisters as icp-cli reads them from the IC's
  certified state, and the controller's `state` and `health` (its subnet and
  settings, which the controller asks the IC about itself) from update calls
  a key sends, and checks them against the module hash and policy the owner
  expects. It reads nothing from the wallet. It cannot read a canister's
  history, which the IC gives only to canisters: the wallet checks the
  controller's history itself before it hands itself over (below).
- **Rotation.** A `SetPolicy` operation replaces the keys, thresholds,
  delays and release signers, with a proof of possession for each new key.
  It needs the sovereign threshold and delay. Executing it raises the policy
  version, which turns every other open operation stale.
- **Passkeys and hardware wallets** are not in this version: verifying a
  WebAuthn assertion or a secp256k1 signature needs another dependency. Each
  key carries no kind field yet; a later key kind is a new variant.

## Operations, thresholds and delays

| Operation | Route | Keys | Delay |
| --- | --- | --- | --- |
| `InstallCode` upgrade with a release our signers signed | Release | normal threshold | normal delay |
| `InstallCode` without such a release, with `skip_pre_upgrade`, or a reinstall | Sovereign | sovereign threshold | sovereign delay |
| `SetPolicy` | Sovereign | sovereign threshold | sovereign delay |
| `ReplaceController` | Sovereign | sovereign threshold | sovereign delay |

The delay starts when the threshold is met. The defaults, which Behrad
chose, are for one owner with three keys (a daily key, a hardware or offline
key, a paper key): normal threshold 1 and delay 48 hours, sovereign
threshold 2 and delay 7 days.

The controller enforces, whatever the policy says:

- 3 to 10 keys, and 2 ≤ sovereign threshold ≤ number of keys − 1: one stolen
  key never installs code, changes the policy or replaces the controller,
  and one lost key never blocks a key rotation or a repair. So one or two
  keys are never a policy.
- 1 ≤ normal threshold < sovereign threshold.
- A normal delay of at least 24 hours, a sovereign delay of at least 72
  hours and longer than the normal delay, and neither over 30 days.
- Release signers apart from the recovery keys: a key that both signed a
  release and approved it would make any module one key's choice.
- No operation runs without at least one recovery key: there is no unsigned
  or automatic fallback, and the release route still needs the normal
  threshold of owner keys on top of the release signatures.
- An operation expires after its delay could end and within 60 days, and
  one key proposes at most 8 operations in 24 hours: the controller keeps
  every operation, so this bounds how fast one key grows its storage.

**Cancellation.** Any key can cancel an open operation, except a `SetPolicy`
that removes that key. So stolen keys below the sovereign threshold can
delay the others by one sovereign delay, the time it takes them to remove
those keys in one `SetPolicy`, and cannot block them for good. Meanwhile a
stolen key can make the owners sign again: an operation names the
controller's next nonce, so a key that proposes first takes the nonce the
owners signed for, at most 8 times a day.

Someone who holds the sovereign threshold of keys can take over. Any other
key cancels an install they propose, so they must first remove every other
key with a `SetPolicy`, which no removed key can cancel, and which can also
lower the delays to their floors and name a release signer they hold. Once
it runs they install on the release route after the new normal delay. That
is at least 4 days in all (72 hours, then 24), and 8 with the defaults (7
days, then 24 hours), with two operations pending in turn, while the wallet
still runs and the owners can move their funds out of it.

**Release route.** The policy holds the release signer keys and threshold;
the controller checks the manifest's signatures, that it names the same Wasm
hash and chunks as the operation, and that its `tested_from` lists the
module the wallet runs now. The manifest does not name the install
argument, so on the release route the normal threshold of owner keys alone
chooses it; the wallet's `post_upgrade` takes only `WalletArg::Upgrade`. The controller never calls the release
channel: the operation carries the manifest, and whoever executes it uploads
the chunks, so a release channel that is down or gone changes nothing. If
our company, release channel or signers disappear, the owners use the
sovereign route, or rotate the release signer keys to their own with a
`SetPolicy`.

## Precise approval

An operation is a record of the controller id, the wallet id, the policy
version, a nonce, an expiry and one operation kind; its digest is SHA-256
over a fixed byte encoding (in `crates/recovery`, with test vectors, so a
client in another language computes the same digest). An `InstallCode`
names the install mode, `skip_pre_upgrade`, the Wasm's SHA-256, every chunk's
SHA-256, the argument's SHA-256 and whether it takes the release route, and
on the release route the release's version, size, `tested_from` hashes,
notes URL and publication time, so nobody can pair the approved module with
another manifest. `docs/recovery.md` specifies the encoding, and a Python
implementation written from that text alone reproduces the pinned vectors
in CI. An
approval is a signature over `b3wallet-controller/approve`, a NUL and the
digest; a cancel signs `b3wallet-controller/cancel` the same way.

- **Replay**: the nonce must be the controller's next one, an operation runs
  once, and each id names one operation for good.
- **Another controller or wallet**: both ids are in the digest.
- **Substituted artifacts**: each uploaded chunk must hash to the approved
  chunk hash, and the IC checks the assembled Wasm against the approved
  module hash at install; the argument is stored with the operation and its
  hash is in the digest.
- **Duplicate approvals**: one per key.
- **Stale approvals**: an operation approved under an older policy version
  never runs; the expiry ends the rest.

## Recovery execution

The controller recovers the wallet in place, so its canister id, its
threshold key derivation paths (which start from the canister id) and its
ledger accounts stay what they were. Copying state into a new canister would
not move the signing authority, and is not offered.

The controller takes an operation's chunks only once its threshold of
keys approved it, each checked against its approved hash, and holds at most
200 chunks for all open operations, so one key cannot make it pay to store
modules nobody approved. An `InstallCode` runs only once every chunk is
there, and only if the controller can pay for its own calls (100B and 2B
per chunk) and for what the wallet lacks of the 0.35T cycles an install
needs above its freezing reserve; otherwise it refuses with the amounts
before it stops anything. It:

1. for an upgrade, stops the wallet, which waits for every call the wallet
   has in flight to be answered, so no callback of the old code runs in the
   new one, and no transfer is half sent across the install. For a
   reinstall it uninstalls the wallet's code instead, which rejects every
   call the code took and drops the callbacks of every call it made: code
   that keeps a call in flight never lets the wallet stop, so an upgrade of
   it fails after the IC's stop timeout, and only a reinstall replaces it;
2. reads the wallet's cycles again, now that its code can spend nothing, and
   gives it what it lacks of the 0.35T: code that drained the wallet cannot
   keep it from being repaired, since it cannot spend the controller's
   cycles;
3. empties the wallet's chunk store and stages the chunks there: the wallet
   runs no code, so it can neither add to the store nor clear it, and since
   the store is the wallet's own the controller may live on any subnet
   (though a wallet is handed only to a controller on its own, below);
4. installs from them with `install_chunked_code` (an upgrade, or an install
   into the emptied wallet), which the IC rejects whole if the module hash
   differs or the new code's `post_upgrade` or `init` traps;
5. empties the store again and starts the wallet, also when any step
   failed.

A failed step leaves the operation ready until it expires, so the owners can
top up or try again without a new delay; it never runs twice once it ran.
If the controller stops in the middle (for instance out of cycles), its
timer starts the wallet again. Before it sends the install it records the
wallet's change count and its own canister version, which the IC writes
into the change the install makes; the next execution reads the wallet's
canister history and takes the install as run only if that very change is
there. A rejected install changed nothing, and the controller forgets it
sent it.

An upgrade keeps the wallet's stable state: open transfers keep their
ledger `created_at_time` and their signed Bitcoin transactions, reserved
outputs stay reserved, and the transfer history stays the record the next
transfer checks. A reinstall erases all of that and is only on the sovereign
route. Funds stay where they were: an account's address comes from the
wallet's canister id and the account's index, so the owners install with a
new claim key, claim the wallet again and add their accounts in the same
order to see and spend them (a PocketIC test does exactly that with an
ICRC-1 ledger). Transfers that were open must be reconciled with the
ledgers and the Bitcoin network by hand. The controller takes and restores no snapshots: restoring local
state does not undo a transfer, and could send one twice.

## What each kind of failure gets

| Failure | What recovers it | What does not |
| --- | --- | --- |
| Broken wallet code: pages, API, authorization or upgrade methods | An `InstallCode` on the release route, or the sovereign route if no signed fix exists | Nothing, if the controller itself is broken; the wallet then keeps running as it is |
| Lost recovery keys | The remaining keys rotate them out with `SetPolicy`, if they reach the sovereign threshold | Losing more keys than that leaves the wallet running but never upgradable again |
| Stolen recovery keys | Fewer than the sovereign threshold: cancel their operations, rotate them out. The threshold or more: the delays give the owners time to move funds out through the wallet | The controller cannot tell owners from thieves who hold enough keys |
| A malicious release | Our signers alone cannot install it; the owners' keys and the delay stand in the way, and any key can cancel. One that was installed is replaced by an upgrade, or, if it keeps a call in flight so the wallet never stops, by a reinstall; if it drained the wallet's cycles the controller pays for the install | Funds it moved before it was replaced; after a reinstall, the wallet's members, account list and transfer history, which the owners rebuild |
| The IC unavailable or a subnet halted | Nothing here. The wallet, its controller and its keys live on the IC | Funds are reachable only when the IC runs |
| Lost Internet Identity for the wallet's origin | Not this mechanism: members are the wallet's own business. A repair could add a member, but only by installing code that does so | |

## Migrating a wallet that controls itself

A wallet that controls itself can only give control away through its own
code, so migration needs a wallet release with a new action,
`HandOverControl { controller, controller_module_hash,
controller_policy_sha256 }`, under the `SetPolicy` rule (the wallet's
strongest quorum and timelock):

1. The owner creates the controller on the wallet's subnet with the IC's
   default settings, signs a proof of possession with each recovery key,
   installs it once with the wallet's id and the policy, and removes every
   controller from it.
2. `recovery-tool check --before-hand-over` checks the controller's module
   hash, that it has no controller, that it serves this wallet with this
   policy and has taken no operation yet, and its settings, and prints the
   three values the hand-over needs.
3. An owner proposes `HandOverControl` with them on the wallet's new
   Recovery card; the wallet's quorum approves; the timelock runs.
4. On `execute`, the wallet reads its own controllers first: if they are
   already exactly the controller, the action is done. Otherwise it checks
   through `canister_info` that the controller runs the named module, has no
   controller, and that its whole history is its creation, one install of
   that module and changes of its controllers (no reinstall, upgrade,
   uninstall or snapshot load, any of which could leave it holding state its
   own `init` did not write; more than 20 changes fail closed). It asks the
   controller that it serves this wallet with the named policy, has not
   been replaced, and is as its install left it (policy version 1, next
   nonce 1). It asks the controller's `health` that it runs on the wallet's
   own subnet, so it adds no subnet the wallet depends on, and that its
   settings, which nobody can change once it has no controller, cannot
   strand the wallet: no controller, a freezing threshold of 30 to 365 days,
   no compute or memory allocation, a reserved cycles limit of at least the
   IC's default 5T and a Wasm memory limit of none or at least 1 GiB. Then
   it sets its controllers to exactly `[controller]` in one call.
5. `recovery-tool check` checks the wallet's controllers.

From then on the wallet's own `Upgrade` action fails, since the wallet is
no longer its own controller; upgrades go through the recovery keys. An
`Upgrade` action the owners approved before the hand-over and execute after
it fails the same way; one executed before it installs that release first,
and the hand-over then runs under the new code's checks.
`docs/recovery.md` is the step-by-step guide, rehearsed end to end with
icp-cli 1.7.0 on its local network.

| Interrupted at | State | What happens next |
| --- | --- | --- |
| Before step 4 | Wallet controls itself; the controller has no power over it | Nothing to undo; an unused controller can be left |
| A check in step 4 fails | Nothing changed | The action ends `Failed` with the reason |
| The `update_settings` call fails | Nothing changed; the call is atomic | The action stays ready |
| The wallet traps after the call succeeded | The controller controls the wallet; the action is not marked | The next `execute` sees its controllers are the controller's and ends it `Executed` |
| After step 4 | The controller is the only controller | The wallet can no longer change its controllers |

No step adds a controller other than the controller, and the call removes
the wallet itself, so no developer authority is left behind. A wallet whose
`HandOverControl` was never executed stays as it is now.

New wallets from the factory should come with a controller from the start:
the factory creates both canisters on one subnet, installs the controller in
a state where the claim key enrolls its recovery keys, makes it the wallet's
only controller and leaves the controller with none. That is a separate
step after this one.

## Evolving the controller

The controller has no controller, so its code never changes. Keys,
thresholds, delays and release signers change by `SetPolicy`. A new
controller version takes over by `ReplaceController`, on the sovereign route:
the old controller makes the same checks of the new one as the wallet's
hand-over, history, freshness and settings included, with its own subnet
standing for the wallet's, and then sets the wallet's controllers to
exactly the new one. It takes the wallet as handed over without another
look only if it sent that change itself and lost the answer. Nobody but the owners' keys can
start either, so no developer backdoor appears when the controller evolves.

## Cycles

Anyone can top up either canister (`icp canister top-up`, or the cycles
minting canister's `notify_top_up`), with no controller needed. Wallet code
can spend the wallet's cycles but never the controller's, so the controller
pays what the wallet lacks for an install once the wallet's code has
stopped, and before it stops anything checks that it could. It keeps chunks
only while their operation is open. Measured on
icp-cli's local network, which charges as a 13-node subnet: a 4.9 MB wallet
upgrade (5 chunks) cost the controller 33B cycles from proposal to
execution.

## What remains trusted

- **ICP**: the subnet's nodes, its protocol and NNS governance, which can
  change the protocol that runs every canister. A halted subnet halts both.
- **The recovery keys**: the sovereign threshold of keys can install any
  code after the delays. Keys held by one person on one device are one
  authority, whatever the count.
- **The controller's code**: it is immutable, so a bug in it stays until the
  owners replace it, which needs its `ReplaceController` to work. It has
  had no external audit.
- **The release signers**, for the release route only and only if the
  policy names any: the release threshold of them and the normal threshold
  of owner keys can install a release after the normal delay.
- **The recovery tool** and the computer it runs on, which show what is
  signed.

## Residual risks

Passing tests do not make this production-ready. What is still open:

- **No external audit** of the controller, the wallet's hand-over or the
  recovery tool. One independent review was done inside this work; its
  findings and what changed are listed below.
- **No mainnet rehearsal.** Everything ran on PocketIC and on icp-cli's
  local network, where the costs above were measured.
- **The controller is immutable.** A bug in it is fixed only by moving to a
  new controller, which needs its `ReplaceController` path to work. A bug
  there leaves the wallet running on its last code, not stranded with its
  funds, but never upgradable again.
- **Key custody is the owners'.** The sovereign threshold of keys can
  install anything after the delays; the controller cannot tell owners from
  thieves who hold enough keys. Two keys on one device are one authority.
  Nothing checks how the keys are kept.
- **The delays help only someone watching.** Nothing tells the owners that
  an operation was proposed; the wallet does not show the controller's
  operations yet. Someone holding the sovereign threshold of keys needs at
  least 4 days (8 with the defaults), but owners who never look will not
  notice.
- **A stolen key below the threshold is a nuisance until removed.** It can
  cancel the owners' operations, and take the nonce they signed for so they
  must sign again, up to 8 times a day, until a `SetPolicy` removes it one
  sovereign delay later.
- **Release signers** on the release route, if the owners trusted any: the
  release threshold of signers and the normal threshold of owner keys can
  install after the normal delay. The signers do not sign the install
  argument; the normal threshold of keys chooses it.
- **The controller's hash is only as good as its build.** The build is the
  same in any directory on Linux with the pinned toolchain; other operating
  systems have not been compared. An owner who does not compare the hash
  with an independent build trusts whoever gave them the Wasm.
- **What `check` cannot see.** Owners cannot read `canister_info`, so the
  client relies on the wallet's own history check at hand-over. It reads a
  controller's settings from the controller's own `health`, and it cannot
  tell the wallet's subnet unless the owner names it; the wallet checks
  both itself before it hands itself over. The settings limits (30 to 365
  days of freezing threshold, the IC's default limits) are our choice.
- **A reinstall loses the wallet's state.** Code that never lets the wallet
  stop can only be replaced by a reinstall: the owners keep their funds but
  claim the wallet again, add their accounts again in the same order, and
  reconcile open transfers by hand.
- **Canisters can spend the controller's cycles.** Any canister can call its
  reads and `health`, paying for its own call while the controller pays for
  running it. Ingress from anyone but a key is dropped before it costs
  anything.
- **New kinds of IC history changes fail closed.** A kind of change the
  wallet's and controller's code does not know makes their history checks
  refuse, which stops a hand-over, a `ReplaceController` or the retry of a
  lost install until a release that knows it.
- **Rented subnets.** A subnet's admins can stop, uninstall and delete any
  canister on it, though not install code. A wallet should not live on such
  a subnet; the same-subnet rule only keeps its controller from adding
  another.
- **Paths no test drives:** the controller's timer, which takes a step
  that ran over an hour as dead and starts a wallet the controller left
  stopped (tests cover `start_wallet` and executions that fail and start
  the wallet again, not a controller whose execution died part way); the
  retry of an install or hand-over whose answer was lost, which needs a
  callback that traps after the IC answered; and the Recovery card in a
  browser (its hash parsing is tested, and the hand-over it proposes is
  tested through the API; nobody has clicked it with a signed-in owner).
- **The IC itself.** A halted subnet halts the wallet and its controller,
  and neither can move the wallet to another subnet: that would need a new
  operation, and so a new controller.
- **Factory wallets** still come without a controller; a later step must
  give each one a controller from its creation.
- **Passkeys and hardware wallets** are not recovery keys yet.

## Independent review

A reviewer who had not written the code read the controller, the hand-over
and the encoding against this ADR. What it found, and what changed:

| Finding | Change |
| --- | --- |
| High: wallet code that keeps a call in flight never lets the wallet stop, and code that spends the wallet's cycles keeps it below what an install needs, so hostile code could block its own repair | A reinstall uninstalls the code instead of stopping it; the controller pays what the wallet lacks once its code is stopped or gone (two PocketIC tests: a held call and a drained wallet) |
| A two-key policy had to be 1 of 2, so one stolen key took everything | 3 to 10 keys, sovereign threshold 2 or more, normal below sovereign, sovereign delay longer |
| A recovery key that was also a release signer made the sovereign route one key's | Release signers and recovery keys must be apart; the attacker times above corrected |
| One key could store operations without end with expiries a nanosecond away | Expiry after the delay; 8 proposals per key a day |
| Strangers could send reads as updates, which the controller pays for | `inspect_message` takes messages from keys only |
| A creator's settings (freezing threshold, limits) or another subnet could strand a wallet | `health`; the hand-over and `ReplaceController` check settings, the subnet and a controller fresh from its install |
| A retried install could take another operation's install of the same module for its own | A rejected install is forgotten; the change must carry the controller's version from the send |
| `ReplaceController` took any wallet already controlled by the target as handed over | Only after it sent the change itself |
| Labels could imitate other labels with look-alike or direction characters | Printable ASCII, unlike every other label |

It also noted that the normal threshold chooses the release route's
argument, that an `Upgrade` action can sit beside a `HandOverControl`, and
that new IC change kinds fail closed; the sections above and the residual
risks say so.

## Options considered

| Option | For | Against |
| --- | --- | --- |
| **A. Per-wallet immutable controller held by owner keys** | No wallet code and no developer can change it; recovery needs nothing from the wallet | One more canister per wallet to fund; a controller bug can only be fixed by replacing it |
| B. Keep 0005 (no rescue path) | Least code | A release that breaks upgrades strands the wallet |
| C. Orbit's upgrader | Exists | The wallet controls it, NNS root co-controls it, and recovery authority is set by the wallet |
| D. A self-upgrading controller | Bugs can be fixed in place | Its own upgrade path can break as the wallet's can; harder to verify |
| E. A shared controller for many wallets | One canister | One bug or one key compromise reaches every wallet |
