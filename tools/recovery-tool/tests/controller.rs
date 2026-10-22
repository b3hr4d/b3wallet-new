//! Drives a real recovery controller in PocketIC with what `recovery-tool`
//! writes, the way owners recover a wallet with nothing but the tool,
//! icp-cli and their keys: no wallet page, wallet API or release service.
//!
//! `icp_cli_sends_what_the_tool_writes` sends every call with icp-cli (the
//! binary `ICP_CLI` names) through PocketIC's HTTP gateway, signed by each
//! recovery key imported as an icp-cli identity, and reads what anyone can
//! read with `icp canister status --public`. The other test sends the same
//! calls to PocketIC directly, encoding the tool's Candid text with
//! `candid/controller.did` as icp-cli does.
//!
//! Both need the PocketIC server and the controller and caller Wasm, so
//! they are ignored by default; CI runs them with `--include-ignored`.

use candid::types::{Type, TypeEnv, TypeInner};
use candid::{CandidType, Principal, decode_one, encode_args};
use candid_parser::utils::CandidSource;
use ed25519_dalek::SigningKey;
use ed25519_dalek::pkcs8::EncodePrivateKey;
use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
use pocket_ic::{PocketIc, PocketIcBuilder, Time};
use recovery::controller::{ControllerError, OperationStatus, OperationView};
use recovery::{DAY_NS, HOUR_NS, principal_of, sha256};
use recovery_tool::{Proposal, from_hex, hex, read_state};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

const INTERFACE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../candid/controller.did");

fn read_wasm(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../../target/wasm32-unknown-unknown/release/{name}.wasm",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {path} ({e}); build it with `cargo build -p {name} --target \
             wasm32-unknown-unknown --release`"
        )
    })
}

/// `wasm` with a custom section, so it is another module that runs the
/// same code.
fn tagged(wasm: &[u8], tag: &str) -> Vec<u8> {
    let name = b"b3wallet-test-release";
    let mut module = wasm.to_vec();
    module.push(0);
    let size = 1 + name.len() + tag.len();
    assert!(size < 128 && name.len() < 128);
    module.push(size as u8);
    module.push(name.len() as u8);
    module.extend_from_slice(name);
    module.extend_from_slice(tag.as_bytes());
    module
}

struct Dir(PathBuf);

impl Dir {
    /// A new directory outside the repository, so icp-cli finds no project
    /// above it.
    fn new(name: &str) -> Dir {
        let dir = std::env::temp_dir().join(format!("recovery-tool-{name}-{}", std::process::id()));
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
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[track_caller]
fn tool(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_recovery-tool"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "recovery-tool {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// The controller's interface, to encode the tool's text as icp-cli does.
struct Interface {
    env: TypeEnv,
    actor: Type,
}

impl Interface {
    fn load() -> Interface {
        let (env, actor) = CandidSource::File(Path::new(INTERFACE)).load().unwrap();
        Interface {
            env,
            actor: actor.unwrap(),
        }
    }

    fn init(&self, text: &str) -> Vec<u8> {
        let TypeInner::Class(types, _) = self.env.trace_type(&self.actor).unwrap().as_ref().clone()
        else {
            panic!("the controller takes an init argument");
        };
        self.encode(text, &types)
    }

    fn args(&self, method: &str, text: &str) -> Vec<u8> {
        let types = self
            .env
            .get_method(&self.actor, method)
            .unwrap()
            .args
            .clone();
        self.encode(text, &types)
    }

    fn encode(&self, text: &str, types: &[Type]) -> Vec<u8> {
        candid_parser::parse_idl_args(text)
            .unwrap()
            .to_bytes_with_types(&self.env, types)
            .unwrap()
    }
}

/// icp-cli, its own home with each recovery key imported as an identity,
/// and PocketIC's HTTP gateway once the test connects it.
struct IcpCli {
    binary: String,
    home: Dir,
    url: Option<String>,
    root_key: String,
}

/// A wallet stand-in that its controller controls, and the controller,
/// installed and configured with nothing but the tool's output. Calls go
/// through icp-cli once it is connected, and to PocketIC's API before.
struct Setup {
    pic: PocketIc,
    interface: Interface,
    icp: Option<IcpCli>,
    dir: Dir,
    wallet: Principal,
    controller: Principal,
    controller_hash: [u8; 32],
}

/// The keys of the test: 1, 2 and 3 enrolled, 4 added later.
const SEEDS: [u8; 4] = [1, 2, 3, 4];

impl Setup {
    /// With `start`, PocketIC's clock starts at that time instead of now.
    fn new(name: &str, icp_cli: Option<String>, start: Option<u64>) -> Setup {
        let mut builder = PocketIcBuilder::new()
            .with_nns_subnet()
            .with_application_subnet();
        if let Some(start) = start {
            builder = builder.with_initial_time(Time::from_nanos_since_unix_epoch(start));
        }
        let pic = builder.build();
        let dir = Dir::new(name);
        for seed in SEEDS {
            let key = SigningKey::from_bytes(&[seed; 32]);
            dir.write(
                &format!("key-{seed}.pem"),
                key.to_pkcs8_pem(LineEnding::LF).unwrap().as_bytes(),
            );
        }

        // The wallet is any canister: the controller never reads its code.
        let wallet = pic.create_canister();
        pic.add_cycles(wallet, 10_000_000_000_000);
        pic.install_canister(wallet, read_wasm("caller"), encode_args(()).unwrap(), None);

        // The owner creates the controller, enrolls keys 1, 2 and 3 with a
        // proof each, installs it and removes every controller from it.
        let controller = pic.create_canister();
        pic.add_cycles(controller, 10_000_000_000_000);
        let (wallet_text, controller_text) = (wallet.to_text(), controller.to_text());
        let mut proofs = Vec::new();
        for seed in SEEDS {
            let proof = tool(&[
                "proof",
                "--key",
                &dir.path(&format!("key-{seed}.pem")),
                "--controller",
                &controller_text,
                "--wallet",
                &wallet_text,
            ]);
            proofs.push(dir.write(&format!("proof-{seed}.did"), proof));
        }
        let interface = Interface::load();
        let icp = icp_cli.map(|binary| {
            let home = Dir::new(&format!("{name}-home"));
            for seed in SEEDS {
                let args = [
                    "identity",
                    "import",
                    &format!("key-{seed}"),
                    "--from-pem",
                    &dir.path(&format!("key-{seed}.pem")),
                    "--storage",
                    "plaintext",
                ];
                let output = icp_command(&binary, &home, &dir, &args);
                assert!(
                    output.status.success(),
                    "icp {args:?}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            IcpCli {
                binary,
                home,
                url: None,
                root_key: hex(&pic.root_key().unwrap()),
            }
        });
        let controller_wasm = read_wasm("controller");
        let setup = Setup {
            pic,
            interface,
            icp,
            dir,
            wallet,
            controller,
            controller_hash: sha256(&controller_wasm),
        };
        let policy = setup.policy_file("policy.did", &[1, 2, 3]);
        let mut args = vec![
            "init",
            "--controller",
            &controller_text,
            "--wallet",
            &wallet_text,
            "--policy",
            &policy,
        ];
        for proof in &proofs[..3] {
            args.extend(["--proof", proof]);
        }
        let init = setup.interface.init(&tool(&args));
        setup
            .pic
            .install_canister(controller, controller_wasm, init, None);
        setup.pic.set_controllers(controller, None, vec![]).unwrap();
        // What the wallet's HandOverControl does, which the integration
        // tests cover: the controller becomes its only controller.
        setup
            .pic
            .set_controllers(wallet, None, vec![controller])
            .unwrap();
        setup
    }

    fn key_hex(&self, seed: u8) -> String {
        tool(&["key", "--key", &self.dir.path(&format!("key-{seed}.pem"))])
            .lines()
            .next()
            .unwrap()
            .trim_start_matches("Public key: ")
            .to_string()
    }

    /// Writes the policy of keys `seeds`, the release route at 1 key and 24
    /// hours and the sovereign route at 2 keys and 72 hours, the shortest
    /// delays a controller takes.
    fn policy_file(&self, name: &str, seeds: &[u8]) -> String {
        let authorities: Vec<String> = seeds
            .iter()
            .map(|&seed| format!("{}=key {seed}", self.key_hex(seed)))
            .collect();
        let mut args = vec!["policy"];
        for authority in &authorities {
            args.extend(["--authority", authority]);
        }
        args.extend([
            "--normal-threshold",
            "1",
            "--sovereign-threshold",
            "2",
            "--normal-delay",
            "24h",
            "--sovereign-delay",
            "72h",
        ]);
        self.dir.write(name, tool(&args))
    }

    /// Writes the tool's operation with its own arguments `own` and the
    /// fields every operation has, expiring in 10 days.
    fn operation(&self, name: &str, own: &[&str], version: u64, nonce: u64) -> String {
        let (controller, wallet) = (self.controller.to_text(), self.wallet.to_text());
        let (version, nonce) = (version.to_string(), nonce.to_string());
        let expires = (self.now() + 10 * DAY_NS).to_string();
        let mut args = own.to_vec();
        args.extend([
            "--controller",
            &controller,
            "--wallet",
            &wallet,
            "--policy-version",
            &version,
            "--nonce",
            &nonce,
            "--expires-at-ns",
            &expires,
        ]);
        self.dir.write(name, tool(&args))
    }

    fn sign(&self, seeds: &[u8], file: &str) {
        for seed in seeds {
            tool(&[
                "sign",
                "--key",
                &self.dir.path(&format!("key-{seed}.pem")),
                file,
            ]);
        }
    }

    fn connected(&self) -> Option<(&IcpCli, &str)> {
        let icp = self.icp.as_ref()?;
        Some((icp, icp.url.as_deref()?))
    }

    /// Sends the update `method` with the Candid text in `file` as key
    /// `seed`, and returns the reply's bytes, or the rejection.
    fn call(&self, seed: u8, method: &str, file: &str) -> Result<Vec<u8>, String> {
        if self.connected().is_some() {
            return self.icp_call(
                &format!("key-{seed}"),
                method,
                &["--args-file", &self.dir.path(file)],
                false,
            );
        }
        let arg = self.interface.args(method, &self.dir.read(file));
        self.direct(seed, method, arg)
    }

    /// `call`, signed by icp-cli with no network and sent by `icp message
    /// send`, as from a computer that holds the key and never goes online.
    fn call_signed_offline(&self, seed: u8, method: &str, file: &str) -> Result<Vec<u8>, String> {
        self.icp_call(
            &format!("key-{seed}"),
            method,
            &["--args-file", &self.dir.path(file)],
            true,
        )
    }

    fn direct(&self, seed: u8, method: &str, arg: Vec<u8>) -> Result<Vec<u8>, String> {
        let key = SigningKey::from_bytes(&[seed; 32]);
        self.pic
            .update_call(
                self.controller,
                principal_of(key.verifying_key().as_bytes()).unwrap(),
                method,
                arg,
            )
            .map_err(|reject| reject.reject_message)
    }

    /// Sends an update call with icp-cli, and returns the reply's bytes or
    /// what icp-cli said.
    fn icp_call(
        &self,
        identity: &str,
        method: &str,
        args: &[&str],
        offline: bool,
    ) -> Result<Vec<u8>, String> {
        let (icp, url) = self.connected().expect("icp-cli is connected");
        let controller = self.controller.to_text();
        let mut call = vec!["canister", "call", &controller, method];
        call.extend(args);
        call.extend([
            "--candid",
            INTERFACE,
            "--network",
            url,
            "--identity",
            identity,
        ]);
        let output = if offline {
            let signed = self.dir.path("signed.json");
            call.extend(["--root-key", &icp.root_key, "--sign-only", &signed]);
            run_icp(icp, &self.dir, &call);
            icp_output(
                icp,
                &self.dir,
                &["message", "send", &signed, "--yes", "--output", "hex"],
            )
        } else {
            call.extend(["--root-key", "fetch", "--output", "hex"]);
            icp_output(icp, &self.dir, &call)
        };
        if output.status.success() {
            Ok(from_hex(String::from_utf8(output.stdout).unwrap().trim()).unwrap())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    }

    /// Uploads a chunk file, Candid bytes, as key `seed`.
    fn upload(&self, seed: u8, file: &str) -> Vec<u32> {
        let reply = if self.connected().is_some() {
            self.icp_call(
                &format!("key-{seed}"),
                "upload_chunk",
                &["--args-file", file, "--args-format", "bin"],
                false,
            )
        } else {
            self.direct(seed, "upload_chunk", std::fs::read(file).unwrap())
        };
        decode::<Result<Vec<u32>, ControllerError>>(reply.unwrap()).unwrap()
    }

    /// The controller's state, as the hex icp-cli prints with `--output
    /// hex`, from an update call so that the answer went through consensus.
    /// The controller takes messages from its recovery keys only, so key 1
    /// sends it; an anonymous one is refused.
    fn state_file(&self) -> String {
        let reply = if self.connected().is_some() {
            let refused = self.icp_call("anonymous", "state", &["()"], false);
            assert!(
                refused
                    .as_ref()
                    .is_err_and(|said| said.contains("rejected the message")),
                "{refused:?}"
            );
            self.icp_call("key-1", "state", &["()"], false).unwrap()
        } else {
            assert!(
                self.pic
                    .update_call(
                        self.controller,
                        Principal::anonymous(),
                        "state",
                        encode_args(()).unwrap(),
                    )
                    .is_err()
            );
            self.direct(1, "state", encode_args(()).unwrap()).unwrap()
        };
        self.dir.write("state.hex", hex(&reply))
    }

    /// What anyone can read about `canister` from the IC's certified state.
    fn status_file(&self, canister: Principal, module: [u8; 32], name: &str) -> String {
        let json = match self.connected() {
            Some((icp, url)) => run_icp(
                icp,
                &self.dir,
                &[
                    "canister",
                    "status",
                    &canister.to_text(),
                    "--public",
                    "--json",
                    "--network",
                    url,
                    "--root-key",
                    "fetch",
                ],
            ),
            // PocketIC's API gives a module hash only to a controller, and
            // the controller has none: the test states what icp-cli reads.
            None => format!(
                "{{\"id\":\"{canister}\",\"module_hash\":\"0x{}\",\"controllers\":[{}]}}",
                hex(&module),
                self.pic
                    .get_controllers(canister)
                    .iter()
                    .map(|c| format!("\"{c}\""))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        };
        self.dir.write(name, json)
    }

    /// Lets `ns` pass, before icp-cli connects: icp-cli signs for the
    /// computer's clock, and PocketIC's must not run ahead of it.
    fn wait(&self, ns: u64) {
        assert!(self.connected().is_none());
        self.pic.advance_time(Duration::from_nanos(ns));
        self.pic.tick();
    }

    /// Connects icp-cli through PocketIC's HTTP gateway. PocketIC's clock
    /// then follows the computer's.
    fn connect(&mut self) {
        let url = self.pic.make_live(None).to_string();
        self.icp.as_mut().expect("an icp-cli test").url = Some(url);
    }

    fn now(&self) -> u64 {
        self.pic.get_time().as_nanos_since_unix_epoch()
    }

    fn wallet_module(&self) -> Vec<u8> {
        self.pic
            .canister_status(self.wallet, Some(self.controller))
            .unwrap()
            .module_hash
            .unwrap()
    }

    fn check(&self, policy: &str, wallet_module: [u8; 32]) -> String {
        let controller_status =
            self.status_file(self.controller, self.controller_hash, "controller.json");
        let wallet_status = self.status_file(self.wallet, wallet_module, "wallet.json");
        let state = self.state_file();
        let health = self.health_file();
        tool(&[
            "check",
            "--controller",
            &self.controller.to_text(),
            "--wallet",
            &self.wallet.to_text(),
            "--module-hash",
            &hex(&self.controller_hash),
            "--policy",
            &self.dir.path(policy),
            "--controller-status",
            &controller_status,
            "--wallet-status",
            &wallet_status,
            "--state",
            &state,
            "--health",
            &health,
        ])
    }

    /// The controller's answer to `health`, from key 1, as icp-cli prints it
    /// with `--output hex`.
    fn health_file(&self) -> String {
        let reply = if self.connected().is_some() {
            self.icp_call("key-1", "health", &["()"], false).unwrap()
        } else {
            self.direct(1, "health", encode_args(()).unwrap()).unwrap()
        };
        self.dir.write("health.hex", hex(&reply))
    }
}

fn icp_output(icp: &IcpCli, dir: &Dir, args: &[&str]) -> Output {
    icp_command(&icp.binary, &icp.home, dir, args)
}

/// Runs icp-cli with `home` as its home, from `dir`, outside any project,
/// with telemetry off.
fn icp_command(binary: &str, home: &Dir, dir: &Dir, args: &[&str]) -> Output {
    Command::new(binary)
        .args(args)
        .current_dir(&dir.0)
        .env("HOME", &home.0)
        .env("DO_NOT_TRACK", "1")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("ICP_NETWORK")
        .env_remove("ICP_ENVIRONMENT")
        .output()
        .unwrap()
}

#[track_caller]
fn run_icp(icp: &IcpCli, dir: &Dir, args: &[&str]) -> String {
    let output = icp_output(icp, dir, args);
    assert!(
        output.status.success(),
        "icp {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn decode<R: CandidType + for<'de> Deserialize<'de>>(reply: Vec<u8>) -> R {
    decode_one(&reply).unwrap()
}

fn view(reply: Result<Vec<u8>, String>) -> Result<OperationView, ControllerError> {
    decode(reply.expect("the call was not rejected"))
}

/// Two keys repair the wallet, keys 1 and 2 swap key 3 for key 4, and key
/// 4 proposes and cancels an install, each operation prepared, signed and
/// sent with the tool's files. PocketIC's API carries the calls.
#[test]
#[ignore = "needs the PocketIC server and the controller and caller Wasm"]
fn the_tools_files_drive_a_controller() {
    let setup = Setup::new("direct", None, None);
    let caller = read_wasm("caller");
    let check = setup.check("policy.did", sha256(&caller));
    assert!(check.contains("Everything matches."), "{check}");

    // A repair on the sovereign route: no release vouches for it.
    let next = tagged(&caller, "repair");
    let next_path = setup.dir.write("next.wasm", &next);
    let arg = setup.dir.write("arg.bin", encode_args(()).unwrap());
    let install = setup.operation(
        "install.did",
        &["install", "--wasm", &next_path, "--arg", &arg],
        1,
        1,
    );
    setup.sign(&[1, 2], &install);
    let proposed = view(setup.call(1, "propose", "install.did")).unwrap();
    assert!(matches!(proposed.status, OperationStatus::Waiting { .. }));
    assert_eq!(proposed.approvals.len(), 2);
    let digest = Proposal::read(&setup.dir.read("install.did"))
        .unwrap()
        .operation
        .digest();
    assert_eq!(proposed.digest.as_slice(), digest);
    let chunks = setup.dir.path("chunks");
    tool(&["chunks", "--wasm", &next_path, "--out", &chunks, &install]);
    assert_eq!(
        setup.upload(2, &format!("{chunks}/chunk-0.bin")),
        Vec::<u32>::new()
    );
    setup.dir.write("execute-1.did", "(1 : nat64)");
    assert!(matches!(
        view(setup.call(3, "execute", "execute-1.did")),
        Err(ControllerError::TimelockActive { .. })
    ));
    setup.wait(72 * HOUR_NS);
    let executed = view(setup.call(3, "execute", "execute-1.did")).unwrap();
    assert!(
        matches!(executed.status, OperationStatus::Executed { .. }),
        "{executed:?}"
    );
    assert_eq!(setup.wallet_module(), sha256(&next));

    // Key 4 replaces key 3: key 1 proposes with its approval, and key 2
    // signs the same file later and sends every approval in it, key 1's
    // included. Key 3 cannot cancel its own removal.
    let next_policy = setup.policy_file("next-policy.did", &[1, 2, 4]);
    let rotation = setup.operation(
        "rotation.did",
        &[
            "set-policy",
            "--policy",
            &next_policy,
            "--proof",
            &setup.dir.path("proof-4.did"),
        ],
        1,
        2,
    );
    setup.sign(&[1], &rotation);
    let proposed = view(setup.call(1, "propose", "rotation.did")).unwrap();
    assert_eq!(proposed.status, OperationStatus::Approving);
    setup.sign(&[2], &rotation);
    setup
        .dir
        .write("approve-2.did", tool(&["approval", &rotation]));
    let approved = view(setup.call(2, "approve", "approve-2.did")).unwrap();
    assert!(matches!(approved.status, OperationStatus::Waiting { .. }));
    assert_eq!(approved.approvals.len(), 2);
    setup.dir.write(
        "cancel-3.did",
        tool(&["cancel", "--key", &setup.dir.path("key-3.pem"), &rotation]),
    );
    assert!(matches!(
        view(setup.call(3, "cancel", "cancel-3.did")),
        Err(ControllerError::Forbidden(_))
    ));
    setup.wait(72 * HOUR_NS);
    setup.dir.write("execute-2.did", "(2 : nat64)");
    // Key 4 is no key until the rotation runs, so the controller refuses
    // its call before reading it.
    assert!(setup.call(4, "execute", "execute-2.did").is_err());
    let executed = view(setup.call(1, "execute", "execute-2.did")).unwrap();
    assert!(matches!(executed.status, OperationStatus::Executed { .. }));
    let check = setup.check("next-policy.did", sha256(&next));
    assert!(check.contains("policy version 2, next nonce 3"), "{check}");

    // Key 3 is out. Key 4 proposes an install with its approval, and
    // cancels it.
    assert!(setup.call(3, "execute", "execute-2.did").is_err());
    let again = setup.operation(
        "again.did",
        &["install", "--wasm", &next_path, "--arg", &arg],
        2,
        3,
    );
    setup.sign(&[4], &again);
    let proposed = view(setup.call(4, "propose", "again.did")).unwrap();
    assert_eq!(proposed.approvals.len(), 1);
    setup.dir.write(
        "cancel-4.did",
        tool(&["cancel", "--key", &setup.dir.path("key-4.pem"), &again]),
    );
    let cancelled = view(setup.call(4, "cancel", "cancel-4.did")).unwrap();
    assert!(matches!(
        cancelled.status,
        OperationStatus::Cancelled { .. }
    ));
}

/// The same recovery with icp-cli carrying every call: `propose`,
/// `approve` (signed with no network and sent by `icp message send`),
/// `upload_chunk`, `cancel`, `execute` and `state`, and `icp canister status
/// --public` reading both canisters.
///
/// icp-cli signs for the computer's clock, so PocketIC's never runs ahead
/// of it: the repair is proposed and approved through PocketIC's API 80
/// hours in the past, and icp-cli connects once PocketIC's clock caught up,
/// when the repair's 72-hour delay is over.
#[test]
#[ignore = "needs icp-cli (ICP_CLI), the PocketIC server and the controller and caller Wasm"]
fn icp_cli_sends_what_the_tool_writes() {
    let binary = std::env::var("ICP_CLI")
        .expect("ICP_CLI must name the icp-cli binary, version 1.7.0 or later");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let mut setup = Setup::new("icp-cli", Some(binary), Some(now - 80 * HOUR_NS));
    let caller = read_wasm("caller");
    let next = tagged(&caller, "repair");
    let next_path = setup.dir.write("next.wasm", &next);
    let arg = setup.dir.write("arg.bin", encode_args(()).unwrap());
    let install = setup.operation(
        "install.did",
        &["install", "--wasm", &next_path, "--arg", &arg],
        1,
        1,
    );
    setup.sign(&[1, 2], &install);
    view(setup.call(1, "propose", "install.did")).unwrap();
    let chunks = setup.dir.path("chunks");
    tool(&["chunks", "--wasm", &next_path, "--out", &chunks, &install]);
    assert_eq!(
        setup.upload(1, &format!("{chunks}/chunk-0.bin")),
        Vec::<u32>::new()
    );

    setup.connect();
    let check = setup.check("policy.did", sha256(&caller));
    assert!(check.contains("Everything matches."), "{check}");
    setup.dir.write("execute-1.did", "(1 : nat64)");
    let executed = view(setup.call(3, "execute", "execute-1.did")).unwrap();
    assert!(
        matches!(executed.status, OperationStatus::Executed { .. }),
        "{executed:?}"
    );
    let check = setup.check("policy.did", sha256(&next));
    assert!(
        check.contains(&format!(
            "module {}, controllers [{}]",
            hex(&sha256(&next)),
            setup.controller
        )),
        "{check}"
    );

    // A rotation proposed with no approvals, approved by a message key 2
    // signed with no network; key 3 cannot cancel its own removal, and key
    // 1 cancels it.
    let next_policy = setup.policy_file("next-policy.did", &[1, 2, 4]);
    let rotation = setup.operation(
        "rotation.did",
        &[
            "set-policy",
            "--policy",
            &next_policy,
            "--proof",
            &setup.dir.path("proof-4.did"),
        ],
        1,
        2,
    );
    let proposed = view(setup.call(1, "propose", "rotation.did")).unwrap();
    assert_eq!(proposed.status, OperationStatus::Approving);
    setup.sign(&[1, 2], &rotation);
    setup
        .dir
        .write("approve-2.did", tool(&["approval", &rotation]));
    let approved = view(setup.call_signed_offline(2, "approve", "approve-2.did")).unwrap();
    assert!(
        matches!(approved.status, OperationStatus::Waiting { .. }),
        "{approved:?}"
    );
    for seed in [3, 1] {
        setup.dir.write(
            &format!("cancel-{seed}.did"),
            tool(&[
                "cancel",
                "--key",
                &setup.dir.path(&format!("key-{seed}.pem")),
                &rotation,
            ]),
        );
    }
    assert!(matches!(
        view(setup.call(3, "cancel", "cancel-3.did")),
        Err(ControllerError::Forbidden(_))
    ));
    let cancelled = view(setup.call(1, "cancel", "cancel-1.did")).unwrap();
    assert!(matches!(
        cancelled.status,
        OperationStatus::Cancelled { .. }
    ));

    // Another install, its chunk uploaded and its execution refused while
    // its delay runs. Key 4 was never enrolled, so its calls are refused.
    let later = tagged(&caller, "later");
    let later_path = setup.dir.write("later.wasm", &later);
    let again = setup.operation(
        "again.did",
        &["install", "--wasm", &later_path, "--arg", &arg],
        1,
        3,
    );
    setup.sign(&[2, 3], &again);
    assert!(setup.call(4, "propose", "again.did").is_err());
    let proposed = view(setup.call(2, "propose", "again.did")).unwrap();
    assert!(matches!(proposed.status, OperationStatus::Waiting { .. }));
    let chunks = setup.dir.path("later-chunks");
    tool(&["chunks", "--wasm", &later_path, "--out", &chunks, &again]);
    assert_eq!(
        setup.upload(3, &format!("{chunks}/chunk-0.bin")),
        Vec::<u32>::new()
    );
    setup.dir.write("execute-3.did", "(3 : nat64)");
    assert!(matches!(
        view(setup.call(2, "execute", "execute-3.did")),
        Err(ControllerError::TimelockActive { .. })
    ));
    let state = read_state(&std::fs::read_to_string(setup.state_file()).unwrap()).unwrap();
    assert_eq!((state.policy_version, state.next_nonce), (1, 4));
}
