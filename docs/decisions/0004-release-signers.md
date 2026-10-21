# 0004: Who signs wallet releases in v0

Status: accepted, 2026-10-21. Behrad chose option A.

**Decision: three Ed25519 keys sign releases, and a release needs any two
of them.**

Every wallet pins the release signer keys and the threshold it was installed
with, and accepts a release only when that many of those keys signed its
manifest. Whoever holds enough keys decides what code the wallets can be
asked to install; the owners' approval and timelock still stand between a
signed release and a wallet.

| Option | Who signs | For | Against |
| --- | --- | --- | --- |
| **A. 2 of 3 keys** | Any two of three keys | One lost or leaked key can neither ship nor block a release | Three keys to keep apart, and two of them at hand for every release |
| B. One key | A single key | Simplest to run | Losing it stops upgrades, and leaking it lets someone ship a release |
| C. An SNS | An SNS DAO approves each release | No key holder can ship alone | v0 waits until that SNS exists |

The release channel checks the same two of three before it publishes a
release, and `release-tool` signs and checks manifests (see the README). Who
holds the three keys, and where, is still to be settled before mainnet.
