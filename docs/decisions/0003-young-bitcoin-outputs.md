# 0003: When Bitcoin accounts spend outputs younger than 100 blocks

Status: accepted, 2026-10-21. Behrad chose option A.

**Decision: a Bitcoin account spends any output with at least one
confirmation, those with 100 confirmations first and then the largest first.**

The network refuses a transaction that spends a mining reward before the
reward has 100 confirmations. The Bitcoin canister lists a reward from its
first confirmation and does not say that it is one, so the wallet cannot tell
a young reward from any other young output.

| Option | Spends | For | Against |
| --- | --- | --- | --- |
| **A. After 1 block** | Every confirmed output, those 100 blocks old first | Incoming payments are spendable after one confirmation, as in any Bitcoin wallet; only a transfer that needs a young reward can fail | That transfer sends a transaction the network refuses, yet reads `Executed`, and the reward stays reserved until the wallet learns to see that it is still unspent |
| B. After 100 blocks | Outputs with 100 confirmations, and the change of the wallet's own transactions | No transfer can spend a young reward | Every incoming payment waits 100 confirmations, about 17 hours |

Only an account paid straight from a block, by solo mining or some pools, can
receive a mining reward.
