# 0002: How accounts map to ICP and ICRC subaccounts

Status: accepted, 2026-10-21. Behrad chose option A.

**Decision: account index `n` of a chain is the ICRC-1 subaccount that holds
`n` as a 32-byte big-endian number, on the wallet's own principal. Index 0 is
the default subaccount, so the first account of each chain is the plain
principal.**

| Option | Account `n` is | For | Against |
| --- | --- | --- | --- |
| **A. Index** | Subaccount `n`, big-endian | An address ends in its index (`.1`, `.2`); any wallet or explorer finds the accounts again by counting; no hashing to get wrong | Accounts of the wallet and, later, of its extensions need their own index ranges |
| B. Hashed path | SHA-256 of the derivation path, such as `wallet / Icp / n` | One rule for every holder, the same path the threshold keys use | The address shows nothing about the account; recovering accounts needs the exact path encoding |

ICP and ICRC tokens are held by the canister's principal, not by a derived
key, so the subaccount is the only part that distinguishes accounts. Bitcoin
and EVM accounts (step 9 and later) still derive threshold keys from the path
`wallet / chain / index`.
