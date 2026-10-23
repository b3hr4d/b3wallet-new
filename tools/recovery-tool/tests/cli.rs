//! Runs the `recovery-tool` binary the way key holders do, without a
//! network. `controller.rs` sends what it writes to a real controller.

use candid::{Nat, Principal, decode_one, encode_one};
use ed25519_dalek::SigningKey;
use ed25519_dalek::pkcs8::EncodePrivateKey;
use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
use recovery::controller::{ControllerState, Health, UploadChunk};
use recovery::{DAY_NS, InstallMode, OperationKind, Policy, Route, approve_message, principal_of};
use recovery_tool::{Proposal, from_text, hex, read_policy};
use release::{CHUNK_SIZE, ReleaseManifest};
use serde_bytes::ByteBuf;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CONTROLLER: &str = "rrkah-fqaaa-aaaaa-aaaaq-cai";
const WALLET: &str = "ryjl3-tyaaa-aaaaa-aaaba-cai";

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_string()
    }

    fn write(&self, name: &str, bytes: impl AsRef<[u8]>) -> String {
        let path = self.path(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.path(name)).unwrap()
    }

    /// Writes `key-N.pem` for each seed, and returns their public keys in
    /// hex.
    fn keys(&self, seeds: &[u8]) -> Vec<String> {
        seeds
            .iter()
            .map(|&seed| {
                let key = SigningKey::from_bytes(&[seed; 32]);
                let pem = key.to_pkcs8_pem(LineEnding::LF).unwrap();
                self.write(&format!("key-{seed}.pem"), pem.as_bytes());
                hex(key.verifying_key().as_bytes())
            })
            .collect()
    }

    /// The default policy of keys 1, 2 and 3, as `policy.did`.
    fn policy(&self) -> Policy {
        let keys = self.keys(&[1, 2, 3]);
        let output = tool(&[
            "policy",
            "--authority",
            &format!("{}=daily", keys[0]),
            "--authority",
            &format!("{}=hardware", keys[1]),
            "--authority",
            &format!("{}=paper=in the safe", keys[2]),
            "--normal-threshold",
            "1",
            "--sovereign-threshold",
            "2",
        ]);
        assert!(output.status.success(), "{}", stderr(&output));
        self.write("policy.did", &output.stdout);
        read_policy(&stdout(&output)).unwrap()
    }

    /// A Wasm of two full chunks and a short third one.
    fn wasm(&self, name: &str, seed: u8) -> String {
        let wasm: Vec<u8> = (0..2 * CHUNK_SIZE + CHUNK_SIZE / 2)
            .map(|i| (i as u8).wrapping_add(seed))
            .collect();
        self.write(name, wasm)
    }

    fn proposal(&self, name: &str) -> Proposal {
        Proposal::read(&self.read(name)).unwrap()
    }
}

fn tool(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_recovery-tool"))
        .args(args)
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[track_caller]
fn succeeds(args: &[&str]) -> Output {
    let output = tool(args);
    assert!(
        output.status.success(),
        "{args:?} failed: {}",
        stderr(&output)
    );
    output
}

#[track_caller]
fn refused(args: &[&str], reason: &str) {
    let output = tool(args);
    assert_eq!(
        output.status.code(),
        Some(1),
        "{args:?}: {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains(reason),
        "{args:?} said {}, not {reason}",
        stderr(&output)
    );
}

/// The fields every operation has; it expires on 2026-11-01.
const COMMON: [&str; 10] = [
    "--controller",
    CONTROLLER,
    "--wallet",
    WALLET,
    "--policy-version",
    "1",
    "--nonce",
    "4",
    "--expires-at-ns",
    "1793491200000000000",
];

/// Writes the unsigned upgrade of `wasm.wasm` to `install.did`.
fn new_install(dir: &Dir, extra: &[&str]) -> String {
    let wasm = dir.wasm("wasm.wasm", 0);
    let arg = dir.write("arg.bin", b"DIDL\x00\x00");
    let mut args = vec!["install", "--wasm", &wasm, "--arg", &arg];
    args.extend(COMMON);
    args.extend(extra);
    let output = succeeds(&args);
    dir.write("install.did", &output.stdout);
    stderr(&output)
}

#[test]
fn a_key_names_its_public_key_and_principal() {
    let dir = Dir::new("key");
    let public = dir.keys(&[9]).remove(0);
    let output = stdout(&succeeds(&["key", "--key", &dir.path("key-9.pem")]));
    let principal = principal_of(&recovery_tool::from_hex(&public).unwrap()).unwrap();
    assert_eq!(
        output,
        format!("Public key: {public}\nPrincipal:  {principal}\n")
    );
    let not_a_key = dir.write("not-a-key.pem", "-----BEGIN PRIVATE KEY-----\n");
    refused(&["key", "--key", &not_a_key], "PKCS#8");
}

#[test]
fn a_policy_is_checked_and_reads_back() {
    let dir = Dir::new("policy");
    let policy = dir.policy();
    assert_eq!(policy.authorities[2].label, "paper=in the safe");
    assert_eq!(
        (policy.normal_delay_ns, policy.sovereign_delay_ns),
        (2 * DAY_NS, 7 * DAY_NS)
    );
    let keys = dir.keys(&[1, 2, 3]);
    // Three keys, all needed: one lost key would block every repair.
    let mut args = vec!["policy"];
    let authorities: Vec<String> = keys
        .iter()
        .enumerate()
        .map(|(index, key)| format!("{key}=key {index}"))
        .collect();
    for authority in &authorities {
        args.extend(["--authority", authority]);
    }
    args.extend(["--normal-threshold", "2", "--sovereign-threshold", "3"]);
    refused(&args, "one lost key never blocks");
    args.truncate(args.len() - 4);
    // Two keys: one stolen key would be enough, or one lost key too many.
    let mut two = vec!["policy", "--authority", &authorities[0]];
    two.extend(["--authority", &authorities[1]]);
    two.extend(["--normal-threshold", "1", "--sovereign-threshold", "2"]);
    refused(&two, "3 to 10 recovery keys");
    // Labels tell the keys apart.
    let same: Vec<String> = keys.iter().map(|key| format!("{key}=k")).collect();
    let mut twins = vec!["policy"];
    for authority in &same {
        twins.extend(["--authority", authority]);
    }
    twins.extend(["--normal-threshold", "1", "--sovereign-threshold", "2"]);
    refused(&twins, "another key's label");
    args.extend([
        "--normal-threshold",
        "1",
        "--sovereign-threshold",
        "2",
        "--normal-delay",
        "23h",
    ]);
    refused(&args, "at least 24 hours");
}

#[test]
fn init_needs_a_proof_of_every_key_for_this_controller_and_wallet() {
    let dir = Dir::new("init");
    dir.policy();
    let proof = |seed: u8, controller: &str| {
        let output = succeeds(&[
            "proof",
            "--key",
            &dir.path(&format!("key-{seed}.pem")),
            "--controller",
            controller,
            "--wallet",
            WALLET,
        ]);
        dir.write(&format!("proof-{seed}-{controller}.did"), &output.stdout)
    };
    let proofs: Vec<String> = [1, 2, 3].map(|seed| proof(seed, CONTROLLER)).into();
    let policy = dir.path("policy.did");
    let mut args = vec![
        "init",
        "--controller",
        CONTROLLER,
        "--wallet",
        WALLET,
        "--policy",
        &policy,
    ];
    for proof in &proofs[..2] {
        args.extend(["--proof", proof]);
    }
    refused(&args, "recovery key \"paper=in the safe\" needs a proof");
    let elsewhere = proof(3, WALLET);
    args.extend(["--proof", &elsewhere]);
    refused(&args, "does not verify for this controller and wallet");
    args.truncate(args.len() - 2);
    args.extend(["--proof", &proofs[2]]);
    let output = stdout(&succeeds(&args));
    assert!(
        output.contains("variant {\n    Init = record {"),
        "{output}"
    );
}

#[test]
fn keys_sign_the_operation_they_were_shown() {
    let dir = Dir::new("sign");
    let described = new_install(&dir, &[]);
    let proposal = dir.proposal("install.did");
    let digest = hex(&proposal.operation.digest());
    assert!(
        described.contains(&format!("Digest: {digest}")),
        "{described}"
    );
    assert!(described.contains("upgrade the wallet's code, keeping its state"));
    assert!(described.contains("Route: sovereign"));
    assert!(described.contains("2026-11-01 00:00:00 UTC"));
    assert!(proposal.approvals.is_empty());

    let file = dir.path("install.did");
    for seed in [1, 2] {
        dir.keys(&[seed]);
        let output = succeeds(&[
            "sign",
            "--key",
            &dir.path(&format!("key-{seed}.pem")),
            &file,
        ]);
        assert!(stderr(&output).contains(&digest));
    }
    // Signing again replaces the key's approval.
    succeeds(&["sign", "--key", &dir.path("key-1.pem"), &file]);
    let signed = dir.proposal("install.did");
    assert_eq!(signed.operation, proposal.operation);
    assert_eq!(signed.approvals.len(), 2);
    let message = approve_message(&proposal.operation.digest());
    assert!(signed.approvals.iter().all(|approval| recovery::verify(
        &approval.public_key,
        &message,
        &approval.signature
    )));
    let shown = stdout(&succeeds(&["show", &file]));
    assert!(shown.contains("2 approvals in this file"), "{shown}");

    // The argument of approve carries the id and the approvals.
    let approval = stdout(&succeeds(&["approval", &file]));
    let (id, approvals): (u64, Vec<recovery::Signed>) = from_text(
        &approval,
        &[
            <u64 as candid::CandidType>::ty(),
            <Vec<recovery::Signed> as candid::CandidType>::ty(),
        ],
    )
    .unwrap();
    assert_eq!((id, approvals), (4, signed.approvals.clone()));

    // A file changed after it was signed is refused by every command.
    let tampered = dir.write(
        "tampered.did",
        dir.read("install.did")
            .replace("nonce = 4 : nat64", "nonce = 5 : nat64"),
    );
    for command in ["show", "approval"] {
        refused(&[command, &tampered], "changed after it was signed");
    }
    refused(
        &["sign", "--key", &dir.path("key-2.pem"), &tampered],
        "changed after it was signed",
    );
}

#[test]
fn install_modes_and_releases_are_spelled_out() {
    let dir = Dir::new("modes");
    let described = new_install(&dir, &["--reinstall"]);
    assert!(
        described.contains("REINSTALL the wallet's code, ERASING ITS STATE"),
        "{described}"
    );
    let shown = stdout(&succeeds(&["show", &dir.path("install.did")]));
    assert!(shown.contains("ERASING ITS STATE"));
    assert!(matches!(
        dir.proposal("install.did").operation.kind,
        OperationKind::InstallCode(ref install) if install.mode == InstallMode::Reinstall
    ));
    let described = new_install(&dir, &["--skip-pre-upgrade"]);
    assert!(described.contains("without running its pre_upgrade"));
    let output = tool(&[
        "install",
        "--wasm",
        &dir.wasm("wasm.wasm", 0),
        "--arg",
        &dir.path("arg.bin"),
        "--reinstall",
        "--skip-pre-upgrade",
    ]);
    assert_eq!(output.status.code(), Some(2));

    // A release that names the Wasm takes the release route.
    let wasm = std::fs::read(dir.path("wasm.wasm")).unwrap();
    let mut manifest = ReleaseManifest::new(
        "1.2.0".to_string(),
        &wasm,
        vec![[5; 32]],
        "https://example.org/1.2.0".to_string(),
        1,
    );
    manifest.sign(&SigningKey::from_bytes(&[21; 32]));
    let release = dir.write("release.did", release_tool::to_text(&manifest));
    let described = new_install(&dir, &["--release", &release]);
    assert!(described.contains("Route: release. It installs release 1.2.0"));
    let proposal = dir.proposal("install.did");
    assert_eq!(proposal.operation.route(), Route::Release);

    // One that names another Wasm is refused.
    let other = dir.wasm("other.wasm", 1);
    let arg = dir.path("arg.bin");
    let mut args = vec![
        "install",
        "--wasm",
        &other,
        "--arg",
        &arg,
        "--release",
        &release,
    ];
    args.extend(COMMON);
    refused(&args, "does not name the Wasm");
}

#[test]
fn an_install_argument_can_be_candid_text_of_the_wallets_interface() {
    let dir = Dir::new("arg");
    let wasm = dir.wasm("wasm.wasm", 0);
    let interface = concat!(env!("CARGO_MANIFEST_DIR"), "/../../candid/wallet.did");
    let mut args = vec![
        "install",
        "--wasm",
        &wasm,
        "--arg-candid",
        "(variant { Upgrade = null })",
        "--interface",
        interface,
    ];
    args.extend(COMMON);
    let output = succeeds(&args);
    let file = dir.write("install.did", &output.stdout);
    let shown = stdout(&succeeds(&["show", "--interface", interface, &file]));
    assert!(
        shown.contains("Install argument: (variant { Upgrade = null })"),
        "{shown}"
    );
    args[4] = "(variant { Downgrade })";
    refused(&args, "not the init argument");
}

#[test]
fn chunks_are_written_only_for_the_operations_module() {
    let dir = Dir::new("chunks");
    new_install(&dir, &[]);
    let file = dir.path("install.did");
    let out = dir.path("chunks");
    refused(
        &[
            "chunks",
            "--wasm",
            &dir.wasm("other.wasm", 1),
            "--out",
            &out,
            &file,
        ],
        "is not the module operation 4 installs",
    );
    succeeds(&[
        "chunks",
        "--wasm",
        &dir.path("wasm.wasm"),
        "--out",
        &out,
        &file,
    ]);
    let wasm = std::fs::read(dir.path("wasm.wasm")).unwrap();
    for index in 0..3 {
        let bytes = std::fs::read(format!("{out}/chunk-{index}.bin")).unwrap();
        let chunk: UploadChunk = decode_one(&bytes).unwrap();
        assert_eq!((chunk.operation, chunk.index), (4, index));
        let start = index as usize * CHUNK_SIZE;
        let end = wasm.len().min(start + CHUNK_SIZE);
        assert_eq!(chunk.chunk.as_slice(), &wasm[start..end]);
    }
    assert!(!Path::new(&format!("{out}/chunk-3.bin")).exists());
}

#[test]
fn set_policy_and_replace_name_what_they_change() {
    let dir = Dir::new("set-policy");
    let policy = dir.policy();
    dir.keys(&[4]);
    let proof = |controller: &str| {
        let output = succeeds(&[
            "proof",
            "--key",
            &dir.path("key-4.pem"),
            "--controller",
            controller,
            "--wallet",
            WALLET,
        ]);
        dir.write(&format!("proof-{controller}.did"), &output.stdout)
    };
    let policy_file = dir.path("policy.did");
    let mut args = vec!["set-policy", "--policy", &policy_file];
    args.extend(COMMON);
    let wrong = proof(WALLET);
    args.extend(["--proof", &wrong]);
    refused(&args, "does not verify for this controller and wallet");
    let right = proof(CONTROLLER);
    let last = args.len() - 1;
    args[last] = &right;
    let output = succeeds(&args);
    assert!(stderr(&output).contains("replace the whole policy"));
    let proposal = Proposal::read(&stdout(&output)).unwrap();
    assert!(matches!(
        proposal.operation.kind,
        OperationKind::SetPolicy { policy: ref set, ref proofs } if *set == policy && proofs.len() == 1
    ));

    let module = hex(&[3; 32]);
    let mut args = vec![
        "replace",
        "--new-controller",
        "r7inp-6aaaa-aaaaa-aaabq-cai",
        "--module-hash",
        &module,
        "--policy",
        &policy_file,
    ];
    args.extend(COMMON);
    let output = succeeds(&args);
    assert!(stderr(&output).contains("for good"));
    let proposal = Proposal::read(&stdout(&output)).unwrap();
    assert!(matches!(
        proposal.operation.kind,
        OperationKind::ReplaceController { ref policy_sha256, .. }
            if policy_sha256.as_slice() == policy.sha256()
    ));
}

#[test]
fn an_operation_expires_within_sixty_days() {
    let dir = Dir::new("expiry");
    let wasm = dir.wasm("wasm.wasm", 0);
    let arg = dir.write("arg.bin", b"DIDL\x00\x00");
    let mut args = vec!["install", "--wasm", &wasm, "--arg", &arg];
    args.extend(&COMMON[..8]);
    args.extend(["--expires-in", "61d"]);
    refused(&args, "within 60 days");
    let last = args.len() - 1;
    args[last] = "30d";
    let proposal = Proposal::read(&stdout(&succeeds(&args))).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let lifetime = proposal.operation.expires_at_ns - now;
    assert!(lifetime <= 30 * DAY_NS && lifetime > 30 * DAY_NS - 60_000_000_000);
}

#[test]
fn a_cancel_is_signed_for_the_operation() {
    let dir = Dir::new("cancel");
    new_install(&dir, &[]);
    dir.keys(&[3]);
    let output = succeeds(&[
        "cancel",
        "--key",
        &dir.path("key-3.pem"),
        &dir.path("install.did"),
    ]);
    let (id, signed): (u64, recovery::Signed) = from_text(
        &stdout(&output),
        &[
            <u64 as candid::CandidType>::ty(),
            <recovery::Signed as candid::CandidType>::ty(),
        ],
    )
    .unwrap();
    assert_eq!(id, 4);
    let digest = dir.proposal("install.did").operation.digest();
    assert!(recovery::verify(
        &signed.public_key,
        &recovery::cancel_message(&digest),
        &signed.signature
    ));
}

fn status_json(module: Option<&[u8]>, controllers: &[&str]) -> String {
    format!(
        "{{\"id\":\"x\",\"module_hash\":{},\"controllers\":[{}]}}",
        module
            .map(|hash| format!("\"0x{}\"", hex(hash)))
            .unwrap_or_else(|| "null".to_string()),
        controllers
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(",")
    )
}

#[test]
fn check_reads_icp_clis_output_and_names_every_mismatch() {
    let dir = Dir::new("check");
    let policy = dir.policy();
    let module = [9; 32];
    let state = ControllerState {
        wallet: Principal::from_text(WALLET).unwrap(),
        policy_sha256: ByteBuf::from(policy.sha256()),
        policy: policy.clone(),
        policy_version: 1,
        next_nonce: 1,
        replaced_by: None,
        execution: None,
        wallet_stopped: false,
        cycles: Nat::from(5_000_000_000_000u64),
    };
    let health = Health {
        subnet: Principal::from_slice(&[9; 29]),
        controllers: vec![],
        freezing_threshold: Nat::from(2_592_000u64),
        reserved_cycles_limit: Nat::from(5_000_000_000_000u64),
        wasm_memory_limit: Nat::from(3_221_225_472u64),
        memory_allocation: Nat::from(0u8),
        compute_allocation: Nat::from(0u8),
        cycles: Nat::from(5_000_000_000_000u64),
        idle_cycles_burned_per_day: Nat::from(30_000_000u64),
    };
    let check_with = |controller_status: String,
                      wallet_status: String,
                      state: &ControllerState,
                      health: &Health,
                      extra: &[&str]| {
        let mut args = vec![
            "check".to_string(),
            "--controller".to_string(),
            CONTROLLER.to_string(),
            "--wallet".to_string(),
            WALLET.to_string(),
            "--module-hash".to_string(),
            hex(&module),
            "--policy".to_string(),
            dir.path("policy.did"),
            "--controller-status".to_string(),
            dir.write("controller.json", controller_status),
            "--wallet-status".to_string(),
            dir.write("wallet.json", wallet_status),
            "--state".to_string(),
            dir.write("state.hex", hex(&encode_one(state).unwrap())),
            "--health".to_string(),
            dir.write("health.hex", hex(&encode_one(health).unwrap())),
        ];
        args.extend(extra.iter().map(|arg| arg.to_string()));
        tool(&args.iter().map(String::as_str).collect::<Vec<_>>())
    };
    let check = |controller_status: String, wallet_status: String, state: &ControllerState| {
        check_with(controller_status, wallet_status, state, &health, &[])
    };
    let good_controller = status_json(Some(&module), &[]);
    let good_wallet = status_json(Some(&[1; 32]), &[CONTROLLER]);
    let output = check(good_controller.clone(), good_wallet.clone(), &state);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("Everything matches."));

    let cases: Vec<(String, String, ControllerState, &str)> = vec![
        (
            status_json(Some(&[8; 32]), &[]),
            good_wallet.clone(),
            state.clone(),
            "not the module",
        ),
        (
            status_json(Some(&module), &[WALLET]),
            good_wallet.clone(),
            state.clone(),
            "has controllers, who can change it",
        ),
        (
            good_controller.clone(),
            status_json(Some(&[1; 32]), &[CONTROLLER, "2vxsx-fae"]),
            state.clone(),
            "The wallet's controllers are",
        ),
        (
            good_controller.clone(),
            good_wallet.clone(),
            ControllerState {
                wallet: Principal::from_text(CONTROLLER).unwrap(),
                ..state.clone()
            },
            "serves wallet",
        ),
        (
            good_controller.clone(),
            good_wallet.clone(),
            ControllerState {
                policy: Policy {
                    normal_threshold: 2,
                    ..policy.clone()
                },
                ..state.clone()
            },
            "holds another policy",
        ),
        (
            good_controller.clone(),
            good_wallet.clone(),
            ControllerState {
                replaced_by: Some(Principal::anonymous()),
                ..state.clone()
            },
            "acts no more",
        ),
        (
            good_controller.clone(),
            good_wallet.clone(),
            ControllerState {
                wallet_stopped: true,
                ..state.clone()
            },
            "left the wallet stopped",
        ),
    ];
    for (controller_status, wallet_status, state, reason) in cases {
        let output = check(controller_status, wallet_status, &state);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{reason}: {}",
            stdout(&output)
        );
        assert!(stderr(&output).contains(reason), "{}", stderr(&output));
    }
    // Settings that could strand the wallet, and another subnet than the
    // wallet's.
    let refused_with = |health: &Health, extra: &[&str], reason: &str| {
        let output = check_with(
            good_controller.clone(),
            good_wallet.clone(),
            &state,
            health,
            extra,
        );
        assert_eq!(output.status.code(), Some(1), "{reason}");
        assert!(stderr(&output).contains(reason), "{}", stderr(&output));
    };
    refused_with(
        &Health {
            freezing_threshold: Nat::from(400 * 86_400u64),
            ..health.clone()
        },
        &[],
        "freezing threshold",
    );
    let other_subnet = Principal::from_slice(&[8; 29]).to_text();
    refused_with(&health, &["--wallet-subnet", &other_subnet], "subnet");
    let same_subnet = health.subnet.to_text();
    let output = check_with(
        good_controller.clone(),
        good_wallet.clone(),
        &state,
        &health,
        &["--wallet-subnet", &same_subnet],
    );
    assert!(output.status.success(), "{}", stderr(&output));

    // Before the hand-over, the wallet is its own only controller.
    let mut args: Vec<String> = [
        "check",
        "--controller",
        CONTROLLER,
        "--wallet",
        WALLET,
        "--module-hash",
        &hex(&module),
    ]
    .map(String::from)
    .into();
    args.extend([
        "--policy".to_string(),
        dir.path("policy.did"),
        "--controller-status".to_string(),
        dir.write("controller.json", &good_controller),
        "--wallet-status".to_string(),
        dir.write("wallet.json", status_json(Some(&[1; 32]), &[WALLET])),
        "--state".to_string(),
        dir.write("state.hex", hex(&encode_one(&state).unwrap())),
        "--health".to_string(),
        dir.write("health.hex", hex(&encode_one(&health).unwrap())),
    ]);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    refused(&args, "The wallet's controllers are");
    let mut before = args.clone();
    before.push("--before-hand-over");
    // It names what the wallet's Recovery card asks for.
    let named = stdout(&succeeds(&before));
    for line in [
        format!("Controller id     {CONTROLLER}"),
        format!("Its module hash   {}", hex(&[9; 32])),
        format!("Its policy hash   {}", hex(&dir.policy().sha256())),
    ] {
        assert!(named.contains(&line), "{named}");
    }

    // A controller that took an operation is not handed the wallet.
    let busy = ControllerState {
        next_nonce: 2,
        ..state.clone()
    };
    let busy_state = dir.write("busy.hex", hex(&encode_one(&busy).unwrap()));
    let mut busy_args = before.clone();
    let at = busy_args.iter().position(|arg| *arg == "--state").unwrap();
    busy_args[at + 1] = &busy_state;
    refused(&busy_args, "taken operations");

    // The state and health as Candid text, as icp-cli prints them with the
    // interface.
    let text = recovery_tool::to_text((&state,), &[<ControllerState as candid::CandidType>::ty()]);
    assert_eq!(recovery_tool::read_state(&text), Ok(state.clone()));
    let text = recovery_tool::to_text((&health,), &[<Health as candid::CandidType>::ty()]);
    assert_eq!(recovery_tool::read_health(&text), Ok(health.clone()));
}

#[test]
fn usage_errors_exit_with_2() {
    for args in [
        vec![],
        vec!["frobnicate"],
        vec!["key"],
        vec!["key", "--key"],
        vec!["show"],
        vec!["policy", "--normal-threshold", "x"],
    ] {
        let output = tool(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(stderr(&output).contains("Usage:"));
    }
}
