//! What `recovery-tool` reads and writes: Candid text that icp-cli sends as
//! it is, recovery keys as PKCS#8 PEM, and what icp-cli prints about a
//! canister and about the controller.
//!
//! Every file the tool writes is the Candid text of a value or of a call's
//! arguments, with every field named and every number typed, so it parses
//! back without the interface file and reads the same to icp-cli. An
//! operation file is the argument of the controller's `propose`: the
//! operation and the approvals gathered so far.

use candid::types::{Type, TypeEnv};
use candid::utils::{ArgumentDecoder, ArgumentEncoder};
use candid::{CandidType, IDLArgs, Principal, decode_args, encode_args};
use ed25519_dalek::{Signer, SigningKey};
use recovery::controller::{ControllerState, Health, UploadChunk};
use recovery::{
    DAY_NS, HOUR_NS, InstallMode, Operation, OperationKind, Policy, Proof, Route, Signed,
    approve_message, cancel_message, enroll_message,
};
use release::sha256;
use serde_bytes::ByteBuf;
use std::fmt::Write;

pub use release_tool::{from_hex, hex, read_key};

/// The Candid text of `args`, values of `types`.
pub fn to_text<A: ArgumentEncoder>(args: A, types: &[Type]) -> String {
    let bytes = encode_args(args).expect("the tool's values encode as Candid");
    let args = IDLArgs::from_bytes(&bytes)
        .and_then(|args| args.annotate_types(true, &TypeEnv::new(), types))
        .expect("the tool's own encoding decodes with its own types");
    format!("{args}\n")
}

/// Reads Candid text as values of `types`.
pub fn from_text<A: for<'a> ArgumentDecoder<'a>>(text: &str, types: &[Type]) -> Result<A, String> {
    let args = candid_parser::parse_idl_args(text).map_err(|e| format!("not Candid text: {e}"))?;
    let bytes = args
        .to_bytes_with_types(&TypeEnv::new(), types)
        .map_err(|e| format!("not the values expected: {e}"))?;
    decode_args(&bytes).map_err(|e| format!("not the values expected: {e}"))
}

pub fn policy_text(policy: &Policy) -> String {
    to_text((policy,), &[Policy::ty()])
}

pub fn read_policy(text: &str) -> Result<Policy, String> {
    let (policy,): (Policy,) = from_text(text, &[Policy::ty()])?;
    policy.validate()?;
    Ok(policy)
}

pub fn proof_text(proof: &Proof) -> String {
    to_text((proof,), &[Proof::ty()])
}

pub fn read_proof(text: &str) -> Result<Proof, String> {
    let (proof,): (Proof,) = from_text(text, &[Proof::ty()])?;
    Ok(proof)
}

/// An operation and the approvals gathered for it: the argument of
/// `propose`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    pub operation: Operation,
    pub approvals: Vec<Signed>,
}

impl Proposal {
    pub fn text(&self) -> String {
        to_text(
            (&self.operation, &self.approvals),
            &[Operation::ty(), Vec::<Signed>::ty()],
        )
    }

    /// Reads an operation file, and checks the operation's form and that
    /// every approval in it verifies, so a file changed after it was signed
    /// is refused.
    pub fn read(text: &str) -> Result<Proposal, String> {
        let (operation, approvals): (Operation, Vec<Signed>) =
            from_text(text, &[Operation::ty(), Vec::<Signed>::ty()])?;
        operation.check_form()?;
        let proposal = Proposal {
            operation,
            approvals,
        };
        let message = approve_message(&proposal.operation.digest());
        for (index, signed) in proposal.approvals.iter().enumerate() {
            if !recovery::verify(&signed.public_key, &message, &signed.signature) {
                return Err(format!(
                    "approval {index}, by {}, does not verify: the operation changed after it \
                     was signed",
                    hex(&signed.public_key)
                ));
            }
        }
        Ok(proposal)
    }

    /// Adds `key`'s approval, in place of one it gave before.
    pub fn sign(&mut self, key: &SigningKey) {
        let signed = approve(key, &self.operation);
        self.approvals
            .retain(|other| other.public_key != signed.public_key);
        self.approvals.push(signed);
    }

    /// The argument of `approve`: the operation's id and its approvals.
    pub fn approval_text(&self) -> String {
        to_text(
            (self.operation.nonce, &self.approvals),
            &[u64::ty(), Vec::<Signed>::ty()],
        )
    }
}

pub fn approve(key: &SigningKey, operation: &Operation) -> Signed {
    Signed {
        public_key: ByteBuf::from(key.verifying_key().to_bytes()),
        signature: ByteBuf::from(key.sign(&approve_message(&operation.digest())).to_bytes()),
    }
}

/// The argument of `cancel`, signed by `key`.
pub fn cancel_text(key: &SigningKey, operation: &Operation) -> String {
    let signed = Signed {
        public_key: ByteBuf::from(key.verifying_key().to_bytes()),
        signature: ByteBuf::from(key.sign(&cancel_message(&operation.digest())).to_bytes()),
    };
    to_text((operation.nonce, &signed), &[u64::ty(), Signed::ty()])
}

pub fn proof(key: &SigningKey, controller: Principal, wallet: Principal) -> Proof {
    let public_key = key.verifying_key().to_bytes();
    Proof {
        public_key: ByteBuf::from(public_key),
        signature: ByteBuf::from(
            key.sign(&enroll_message(controller, wallet, &public_key))
                .to_bytes(),
        ),
    }
}

/// The principal a key calls the controller as.
pub fn principal(key: &SigningKey) -> Principal {
    recovery::principal_of(key.verifying_key().as_bytes()).expect("a public key is 32 bytes")
}

/// The arguments of `upload_chunk` for each chunk of `wasm`,
/// Candid-encoded.
pub fn chunk_args(operation: u64, wasm: &[u8]) -> Vec<Vec<u8>> {
    release::chunks(wasm)
        .enumerate()
        .map(|(index, chunk)| {
            candid::encode_one(UploadChunk {
                operation,
                index: index as u32,
                chunk: ByteBuf::from(chunk),
            })
            .expect("a chunk encodes as Candid")
        })
        .collect()
}

/// The controller's state, as icp-cli prints the answer of `state`: Candid
/// text, or with `--output hex` the hex of the Candid bytes.
pub fn read_state(text: &str) -> Result<ControllerState, String> {
    let trimmed = text.trim();
    if !trimmed.is_empty() && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return candid::decode_one(&from_hex(trimmed)?)
            .map_err(|e| format!("not the controller's state: {e}"));
    }
    let (state,): (ControllerState,) = from_text(text, &[ControllerState::ty()])
        .map_err(|e| format!("not the controller's state: {e}"))?;
    Ok(state)
}

/// The controller's answer to `health`, as icp-cli prints it: Candid text,
/// or with `--output hex` the hex of the Candid bytes.
pub fn read_health(text: &str) -> Result<Health, String> {
    let trimmed = text.trim();
    if !trimmed.is_empty() && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return candid::decode_one(&from_hex(trimmed)?)
            .map_err(|e| format!("not the controller's health: {e}"));
    }
    let (health,): (Health,) = from_text(text, &[Health::ty()])
        .map_err(|e| format!("not the controller's health: {e}"))?;
    Ok(health)
}

/// What anyone can read from the IC's certified state about a canister, as
/// `icp canister status --public --json` prints it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicStatus {
    pub module_hash: Option<Vec<u8>>,
    pub controllers: Vec<Principal>,
}

pub fn read_status(json: &str) -> Result<PublicStatus, String> {
    #[derive(serde::Deserialize)]
    struct Status {
        module_hash: Option<String>,
        controllers: Vec<String>,
    }
    let status: Status = serde_json::from_str(json)
        .map_err(|e| format!("not the JSON of `icp canister status --public --json`: {e}"))?;
    let module_hash = match status.module_hash {
        Some(text) => Some(from_hex(text.strip_prefix("0x").unwrap_or(&text))?),
        None => None,
    };
    let controllers = status
        .controllers
        .iter()
        .map(|text| Principal::from_text(text).map_err(|e| format!("controller {text}: {e}")))
        .collect::<Result<_, _>>()?;
    Ok(PublicStatus {
        module_hash,
        controllers,
    })
}

/// A duration such as `90m`, `48h` or `7d`, in nanoseconds.
pub fn parse_duration(text: &str) -> Result<u64, String> {
    let refused = || format!("{text} is not a duration such as 90m, 48h or 7d");
    let unit = text.chars().last().ok_or_else(refused)?;
    let number: u64 = text[..text.len() - unit.len_utf8()]
        .parse()
        .map_err(|_| refused())?;
    let unit_ns = match unit {
        'm' => 60 * 1_000_000_000,
        'h' => HOUR_NS,
        'd' => DAY_NS,
        _ => return Err(refused()),
    };
    number.checked_mul(unit_ns).ok_or_else(refused)
}

/// A duration in whole days, hours or minutes, as precise as it is.
pub fn duration_text(ns: u64) -> String {
    let plural = |n: u64, unit: &str| {
        if n == 1 {
            format!("1 {unit}")
        } else {
            format!("{n} {unit}s")
        }
    };
    let minute = 60 * 1_000_000_000;
    if ns.is_multiple_of(DAY_NS) {
        plural(ns / DAY_NS, "day")
    } else if ns.is_multiple_of(HOUR_NS) {
        plural(ns / HOUR_NS, "hour")
    } else if ns.is_multiple_of(minute) {
        plural(ns / minute, "minute")
    } else {
        format!("{ns} ns")
    }
}

/// A time in nanoseconds since 1970 as a UTC date and time.
pub fn utc(ns: u64) -> String {
    let seconds = ns / 1_000_000_000;
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let second = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        second / 3_600,
        second % 3_600 / 60,
        second % 60
    )
}

/// The proleptic Gregorian date of a day counted from 1970-01-01, by Howard
/// Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// What a key holder should read before signing: every field of the
/// operation, in words.
pub fn describe(operation: &Operation) -> String {
    let mut out = String::new();
    writeln!(
        out,
        "Operation {} for recovery controller {} of wallet {},",
        operation.nonce, operation.controller, operation.wallet
    )
    .unwrap();
    writeln!(
        out,
        "under policy version {}, expiring at {}.",
        operation.policy_version,
        utc(operation.expires_at_ns)
    )
    .unwrap();
    match &operation.kind {
        OperationKind::InstallCode(install) => {
            let mode = match install.mode {
                InstallMode::Upgrade {
                    skip_pre_upgrade: false,
                } => "upgrade the wallet's code, keeping its state".to_string(),
                InstallMode::Upgrade {
                    skip_pre_upgrade: true,
                } => "upgrade the wallet's code without running its pre_upgrade, keeping the \
                      state its stable memory holds"
                    .to_string(),
                InstallMode::Reinstall => {
                    "REINSTALL the wallet's code, ERASING ITS STATE: its members, policy, \
                     accounts and transfer history"
                        .to_string()
                }
            };
            writeln!(out, "It would {mode}").unwrap();
            writeln!(
                out,
                "to the module with SHA-256 {}, in {} chunks,",
                hex(&install.wasm_sha256),
                install.chunk_hashes.len()
            )
            .unwrap();
            writeln!(
                out,
                "with an install argument of {} bytes, SHA-256 {}.",
                install.arg.len(),
                hex(&sha256(&install.arg))
            )
            .unwrap();
            match &install.release {
                Some(manifest) => writeln!(
                    out,
                    "Route: release. It installs release {} ({} bytes, notes at {}), published \
                     at {}, tested to upgrade from {} module hashes; the controller checks the \
                     release signers' signatures.",
                    manifest.version,
                    manifest.wasm_size,
                    manifest.notes_url,
                    utc(manifest.published_at_ns),
                    manifest.tested_from.len()
                )
                .unwrap(),
                None => writeln!(
                    out,
                    "Route: sovereign. No release signer vouches for this module: check it \
                     yourself."
                )
                .unwrap(),
            }
        }
        OperationKind::SetPolicy { policy, proofs } => {
            writeln!(
                out,
                "It would replace the whole policy with this one ({} proofs of possession):",
                proofs.len()
            )
            .unwrap();
            out += &describe_policy(policy);
            writeln!(out, "Route: sovereign.").unwrap();
        }
        OperationKind::ReplaceController {
            controller,
            module_hash,
            policy_sha256,
        } => {
            writeln!(
                out,
                "It would hand the wallet to the controller {controller}, for good. The new \
                 controller must run the module with SHA-256 {}, have no controller, serve this \
                 wallet and hold the policy with SHA-256 {}.",
                hex(module_hash),
                hex(policy_sha256)
            )
            .unwrap();
            writeln!(out, "Route: sovereign.").unwrap();
        }
    }
    writeln!(out, "Digest: {}", hex(&operation.digest())).unwrap();
    out
}

/// A policy in words.
pub fn describe_policy(policy: &Policy) -> String {
    let mut out = String::new();
    for authority in &policy.authorities {
        writeln!(
            out,
            "  recovery key {} \"{}\", calling as {}",
            hex(&authority.public_key),
            authority.label,
            recovery::principal_of(&authority.public_key)
                .map(|principal| principal.to_text())
                .unwrap_or_else(|| "nothing: not 32 bytes".to_string())
        )
        .unwrap();
    }
    let route = |route: Route| {
        let requirement = policy.requirement(route);
        format!(
            "{} of {} keys and {}",
            requirement.threshold,
            policy.authorities.len(),
            duration_text(requirement.delay_ns)
        )
    };
    writeln!(
        out,
        "  a signed release needs {}; anything else needs {}",
        route(Route::Release),
        route(Route::Sovereign)
    )
    .unwrap();
    if policy.release_signers.is_empty() {
        writeln!(
            out,
            "  no release signers: every install takes the sovereign route"
        )
        .unwrap();
    } else {
        writeln!(
            out,
            "  releases need {} of the release signers {}",
            policy.release_threshold,
            policy
                .release_signers
                .iter()
                .map(|key| hex(key))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .unwrap();
    }
    writeln!(out, "  policy SHA-256 {}", hex(&policy.sha256())).unwrap();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use recovery::{Authority, InstallCode};

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn policy() -> Policy {
        Policy {
            authorities: (1..=3)
                .map(|seed| Authority {
                    public_key: ByteBuf::from(key(seed).verifying_key().to_bytes()),
                    label: format!("key \"{seed}\" \\ {{x}}"),
                })
                .collect(),
            normal_threshold: 1,
            sovereign_threshold: 2,
            normal_delay_ns: 48 * HOUR_NS,
            sovereign_delay_ns: 7 * DAY_NS,
            release_signers: vec![],
            release_threshold: 0,
        }
    }

    /// Twelve chunks: more than the ten Candid's printer lays out one per
    /// line before it prints a vector on one line.
    fn operation() -> Operation {
        Operation {
            controller: Principal::from_slice(&[0, 0, 0, 0, 0, 0, 0, 1, 1, 1]),
            wallet: Principal::from_slice(&[0, 0, 0, 0, 0, 0, 0, 2, 1, 1]),
            policy_version: 3,
            nonce: u64::MAX,
            expires_at_ns: 1_800_000_000 * 1_000_000_000,
            kind: OperationKind::InstallCode(InstallCode {
                mode: InstallMode::Upgrade {
                    skip_pre_upgrade: true,
                },
                wasm_sha256: ByteBuf::from(vec![7; 32]),
                chunk_hashes: (0..12).map(|i| ByteBuf::from(vec![i; 32])).collect(),
                arg: ByteBuf::from(b"DIDL\x00\x00".to_vec()),
                release: None,
            }),
        }
    }

    #[test]
    fn files_read_back_as_the_same_values() {
        let policy = policy();
        assert_eq!(read_policy(&policy_text(&policy)), Ok(policy.clone()));

        let mut proposal = Proposal {
            operation: operation(),
            approvals: vec![],
        };
        assert_eq!(Proposal::read(&proposal.text()), Ok(proposal.clone()));
        proposal.sign(&key(1));
        proposal.sign(&key(2));
        proposal.sign(&key(1));
        assert_eq!(proposal.approvals.len(), 2);
        assert_eq!(Proposal::read(&proposal.text()), Ok(proposal.clone()));

        let set_policy = Proposal {
            operation: Operation {
                kind: OperationKind::SetPolicy {
                    policy,
                    proofs: vec![proof(
                        &key(4),
                        proposal.operation.controller,
                        proposal.operation.wallet,
                    )],
                },
                ..operation()
            },
            approvals: vec![],
        };
        assert_eq!(Proposal::read(&set_policy.text()), Ok(set_policy));
    }

    #[test]
    fn an_operation_changed_after_signing_is_refused() {
        let mut proposal = Proposal {
            operation: operation(),
            approvals: vec![],
        };
        proposal.sign(&key(1));
        let text = proposal
            .text()
            .replace("policy_version = 3", "policy_version = 4");
        assert!(text.contains("policy_version = 4"), "{text}");
        assert!(
            Proposal::read(&text)
                .unwrap_err()
                .contains("changed after it was signed")
        );
    }

    #[test]
    fn approval_and_cancel_arguments_decode_as_the_controller_reads_them() {
        let mut proposal = Proposal {
            operation: operation(),
            approvals: vec![],
        };
        proposal.sign(&key(2));
        let (id, approvals): (u64, Vec<Signed>) =
            from_text(&proposal.approval_text(), &[u64::ty(), Vec::<Signed>::ty()]).unwrap();
        assert_eq!((id, approvals), (u64::MAX, proposal.approvals.clone()));
        let (id, signed): (u64, Signed) = from_text(
            &cancel_text(&key(3), &proposal.operation),
            &[u64::ty(), Signed::ty()],
        )
        .unwrap();
        assert_eq!(id, u64::MAX);
        assert!(recovery::verify(
            &signed.public_key,
            &cancel_message(&proposal.operation.digest()),
            &signed.signature
        ));
    }

    #[test]
    fn status_json_reads() {
        let status = read_status(
            r#"{"id":"aaaaa-aa","module_hash":"0x0a0b","controllers":["2vxsx-fae"],"x":1}"#,
        )
        .unwrap();
        assert_eq!(status.module_hash, Some(vec![10, 11]));
        assert_eq!(status.controllers, vec![Principal::anonymous()]);
        let empty = read_status(r#"{"module_hash":null,"controllers":[]}"#).unwrap();
        assert_eq!(empty.module_hash, None);
        assert!(read_status(r#"{"module_hash":"0xzz","controllers":[]}"#).is_err());
        assert!(read_status("not json").is_err());
    }

    #[test]
    fn durations_and_times() {
        assert_eq!(parse_duration("48h"), Ok(48 * HOUR_NS));
        assert_eq!(parse_duration("7d"), Ok(7 * DAY_NS));
        assert_eq!(parse_duration("90m"), Ok(90 * 60 * 1_000_000_000));
        for bad in [
            "",
            "h",
            "7",
            "7w",
            "-1d",
            "1.5d",
            "99999999999999999d",
            "7dé",
        ] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
        assert_eq!(duration_text(48 * HOUR_NS), "2 days");
        assert_eq!(duration_text(HOUR_NS), "1 hour");
        assert_eq!(duration_text(90 * 60 * 1_000_000_000), "90 minutes");
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(
            utc(1_709_210_096 * 1_000_000_000),
            "2024-02-29 12:34:56 UTC"
        );
        assert_eq!(
            utc(4_107_542_399 * 1_000_000_000),
            "2100-02-28 23:59:59 UTC"
        );
    }
}
