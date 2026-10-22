# 0005: No rescue path for a release that breaks upgrades

Status: accepted, 2026-10-21. Behrad chose option A. Superseded by
[decision 0009](0009-recovery-controller.md) once it is accepted.

**Decision: a wallet has no rescue path. It stays its own only controller,
and upgrade tests are the gate.**

A wallet upgrades itself, and only to a release its owners approved. If a
release's own upgrade code is broken, so that no later release can install
over it, the wallet stays on that release for good. Its funds stay safe and
usable from it; it only never upgrades again. A release whose `post_upgrade`
traps is a different case: the install fails, the version before it keeps
running, and the wallet marks the upgrade `Failed` (see the self-upgrade flow
in [the design](../design.md#self-upgrade)).

| Option | What it is | For | Against |
| --- | --- | --- | --- |
| **A. None** | The wallet is its own only controller | Nobody but the owners can ever change the wallet's code | A release that breaks its own upgrade method strands the wallets on it |
| B. Guardian | A guardian canister is a second controller and can only reinstall a signed release after a long timelock | A broken release can be replaced | More code, and one more controller every wallet has to trust |

Every release is tested to upgrade from the last two versions, and each
manifest names the module hashes it was tested from; a wallet refuses a
release not tested from the module it runs. A guardian could still be added
before mainnet, as a new decision.
