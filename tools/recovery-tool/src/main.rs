//! Makes, signs and checks the operations of a wallet's recovery controller
//! (decision 0009). It never talks to the network: icp-cli sends what it
//! writes, as a recovery key imported as an icp-cli identity (see
//! docs/recovery.md).

use candid::types::{Type, TypeEnv, TypeInner};
use candid::{CandidType, IDLArgs, Principal};
use candid_parser::utils::CandidSource;
use recovery::controller::{ControllerArg, ControllerInit};
use recovery::{
    Authority, InstallCode, InstallMode, MAX_LIFETIME_NS, Operation, OperationKind, Policy, Proof,
    sha256,
};
use recovery_tool::{
    Proposal, cancel_text, chunk_args, describe, describe_policy, from_hex, hex, parse_duration,
    policy_text, principal, proof, proof_text, read_health, read_key, read_policy, read_proof,
    read_state, read_status, to_text, utc,
};
use release::ReleaseManifest;
use serde_bytes::ByteBuf;
use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

const USAGE: &str = "\
Usage:
  recovery-tool key --key KEY.pem
      Prints the key's public key in hex, and the principal it calls the
      controller as.
  recovery-tool proof --key KEY.pem --controller ID --wallet ID
      Prints the key's proof of possession for this controller and wallet.
  recovery-tool policy --authority PUBLIC_KEY=LABEL... --normal-threshold N
                       --sovereign-threshold N [--normal-delay DURATION]
                       [--sovereign-delay DURATION]
                       [--release-signer PUBLIC_KEY]... [--release-threshold N]
      Prints a policy, and on stderr what it means and its SHA-256. The
      delays default to 48h and 7d.
  recovery-tool init --controller ID --wallet ID --policy POLICY --proof PROOF...
      Prints the controller's install argument, with a proof for every key.
  recovery-tool install OPERATION_OPTIONS --wasm FILE INSTALL_ARG
                        [--skip-pre-upgrade | --reinstall] [--release MANIFEST]
  recovery-tool set-policy OPERATION_OPTIONS --policy POLICY [--proof PROOF]...
  recovery-tool replace OPERATION_OPTIONS --new-controller ID --module-hash HASH
                        --policy POLICY
      Prints an operation with no approvals yet, the argument of propose, and
      on stderr what it does. OPERATION_OPTIONS are --controller ID
      --wallet ID --policy-version N --nonce N and --expires-in DURATION or
      --expires-at-ns NS; the policy version and the next nonce are in the
      controller's state. INSTALL_ARG is --arg FILE, a file of Candid bytes,
      or --arg-candid TEXT --interface DID, Candid text of the init argument
      of the service in DID. A set-policy needs a proof for every new key;
      replace takes the new controller's policy.
  recovery-tool show [--interface DID] OPERATION
      Describes the operation, and checks the approvals in the file. With
      --interface, also prints the install argument as Candid text.
  recovery-tool sign --key KEY.pem OPERATION
      Adds the key's approval to the operation file, in place of one it gave
      before.
  recovery-tool approval OPERATION
      Prints the argument of approve: the operation's id and its approvals.
  recovery-tool cancel --key KEY.pem OPERATION
      Prints the argument of cancel, signed by the key.
  recovery-tool chunks --wasm FILE --out DIR OPERATION
      Checks that the Wasm is the operation's module, and writes the argument
      of upload_chunk for each chunk, as DIR/chunk-N.bin.
  recovery-tool check --controller ID --wallet ID --module-hash HASH
                      --policy POLICY --controller-status FILE
                      --wallet-status FILE --state FILE --health FILE
                      [--wallet-subnet ID] [--before-hand-over]
      Checks, from what icp-cli printed, that the wallet's only controller is
      the controller (with --before-hand-over, the wallet itself, and the
      controller has taken no operation yet), and that the controller runs
      the module, has no controller, serves this wallet with this policy,
      has not handed it over, left it running, and has settings that cannot
      strand it, on the wallet's subnet if --wallet-subnet names it.

Keys are Ed25519 private keys in PKCS#8 PEM, as written by
  openssl genpkey -algorithm ed25519 -out KEY.pem
Public keys and hashes are hex, ids are principals as text, and a DURATION is
a whole number of minutes, hours or days: 90m, 48h, 7d.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage(message)) => {
            eprintln!("recovery-tool: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
        Err(Failure::Refused(message)) => {
            eprintln!("recovery-tool: {message}");
            ExitCode::FAILURE
        }
    }
}

enum Failure {
    Usage(String),
    Refused(String),
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Failure::Refused(message)
    }
}

fn run(args: &[String]) -> Result<(), Failure> {
    let Some((command, rest)) = args.split_first() else {
        return Err(Failure::Usage("no command".to_string()));
    };
    let options = Options::parse(rest)?;
    match command.as_str() {
        "key" => key(options),
        "proof" => write_proof(options),
        "policy" => policy(options),
        "init" => init(options),
        "install" => install(options),
        "set-policy" => set_policy(options),
        "replace" => replace(options),
        "show" => show(options),
        "sign" => sign(options),
        "approval" => approval(options),
        "cancel" => cancel(options),
        "chunks" => write_chunks(options),
        "check" => check(options),
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(Failure::Usage(format!("unknown command {other}"))),
    }
}

fn key(mut options: Options) -> Result<(), Failure> {
    let key = read_key(&read_text(&options.one("--key")?)?)?;
    options.done(0)?;
    println!("Public key: {}", hex(key.verifying_key().as_bytes()));
    println!("Principal:  {}", principal(&key));
    Ok(())
}

fn write_proof(mut options: Options) -> Result<(), Failure> {
    let key = read_key(&read_text(&options.one("--key")?)?)?;
    let controller = principal_option(&mut options, "--controller")?;
    let wallet = principal_option(&mut options, "--wallet")?;
    options.done(0)?;
    print!("{}", proof_text(&proof(&key, controller, wallet)));
    eprintln!(
        "Proof that {} holds key {} for controller {controller} and wallet {wallet}.",
        principal(&key),
        hex(key.verifying_key().as_bytes())
    );
    Ok(())
}

fn policy(mut options: Options) -> Result<(), Failure> {
    let authorities = options
        .all("--authority")
        .iter()
        .map(|text| {
            let (key, label) = text.split_once('=').ok_or_else(|| {
                Failure::Usage(format!("--authority {text} is not PUBLIC_KEY=LABEL"))
            })?;
            Ok(Authority {
                public_key: ByteBuf::from(from_hex(key)?),
                label: label.to_string(),
            })
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    let normal_threshold = number(&options.one("--normal-threshold")?, "--normal-threshold")?;
    let sovereign_threshold = number(
        &options.one("--sovereign-threshold")?,
        "--sovereign-threshold",
    )?;
    let normal_delay_ns = parse_duration(
        &options
            .optional("--normal-delay")?
            .unwrap_or_else(|| "48h".to_string()),
    )?;
    let sovereign_delay_ns = parse_duration(
        &options
            .optional("--sovereign-delay")?
            .unwrap_or_else(|| "7d".to_string()),
    )?;
    let release_signers = options
        .all("--release-signer")
        .iter()
        .map(|text| from_hex(text).map(ByteBuf::from))
        .collect::<Result<Vec<_>, _>>()?;
    let release_threshold = match options.optional("--release-threshold")? {
        Some(text) => number(&text, "--release-threshold")?,
        None => 0,
    };
    options.done(0)?;
    let policy = Policy {
        authorities,
        normal_threshold,
        sovereign_threshold,
        normal_delay_ns,
        sovereign_delay_ns,
        release_signers,
        release_threshold,
    };
    policy.validate()?;
    print!("{}", policy_text(&policy));
    eprint!("{}", describe_policy(&policy));
    Ok(())
}

fn init(mut options: Options) -> Result<(), Failure> {
    let controller = principal_option(&mut options, "--controller")?;
    let wallet = principal_option(&mut options, "--wallet")?;
    let policy = read_policy_file(&options.one("--policy")?)?;
    let proofs = read_proofs(&mut options)?;
    options.done(0)?;
    if !recovery::is_canister_id(&wallet) || wallet == controller {
        return Err(Failure::Refused(
            "the wallet must be a canister other than the controller".to_string(),
        ));
    }
    recovery::check_proofs(&policy, &proofs, &[], controller, wallet)?;
    let arg = ControllerArg::Init(ControllerInit {
        wallet,
        policy,
        proofs,
    });
    print!("{}", to_text((&arg,), &[ControllerArg::ty()]));
    let ControllerArg::Init(init) = &arg;
    eprint!(
        "The install argument of controller {controller} for wallet {wallet}, with this \
         policy:\n{}",
        describe_policy(&init.policy)
    );
    Ok(())
}

/// The fields every operation has.
struct Common {
    controller: Principal,
    wallet: Principal,
    policy_version: u64,
    nonce: u64,
    expires_at_ns: u64,
}

impl Common {
    fn read(options: &mut Options) -> Result<Common, Failure> {
        let controller = principal_option(options, "--controller")?;
        let wallet = principal_option(options, "--wallet")?;
        let policy_version = number(&options.one("--policy-version")?, "--policy-version")?;
        let nonce = number(&options.one("--nonce")?, "--nonce")?;
        let expires_at_ns = match (
            options.optional("--expires-in")?,
            options.optional("--expires-at-ns")?,
        ) {
            (Some(text), None) => {
                let lifetime = parse_duration(&text)?;
                if lifetime == 0 || lifetime > MAX_LIFETIME_NS {
                    return Err(Failure::Refused(format!(
                        "--expires-in {text}: an operation expires within 60 days"
                    )));
                }
                now().saturating_add(lifetime)
            }
            (None, Some(text)) => number(&text, "--expires-at-ns")?,
            _ => {
                return Err(Failure::Usage(
                    "give one of --expires-in and --expires-at-ns".to_string(),
                ));
            }
        };
        Ok(Common {
            controller,
            wallet,
            policy_version,
            nonce,
            expires_at_ns,
        })
    }

    fn operation(self, kind: OperationKind) -> Operation {
        Operation {
            controller: self.controller,
            wallet: self.wallet,
            policy_version: self.policy_version,
            nonce: self.nonce,
            expires_at_ns: self.expires_at_ns,
            kind,
        }
    }
}

fn install(mut options: Options) -> Result<(), Failure> {
    let common = Common::read(&mut options)?;
    let wasm_path = options.one("--wasm")?;
    let wasm = read(&wasm_path)?;
    let arg = install_arg(&mut options)?;
    let skip_pre_upgrade = options.flag("--skip-pre-upgrade");
    let reinstall = options.flag("--reinstall");
    let release = options
        .optional("--release")?
        .map(|path| read_manifest(&path))
        .transpose()?;
    options.done(0)?;
    let mode = match (skip_pre_upgrade, reinstall) {
        (false, false) => InstallMode::Upgrade {
            skip_pre_upgrade: false,
        },
        (true, false) => InstallMode::Upgrade {
            skip_pre_upgrade: true,
        },
        (false, true) => InstallMode::Reinstall,
        (true, true) => {
            return Err(Failure::Usage(
                "give at most one of --skip-pre-upgrade and --reinstall".to_string(),
            ));
        }
    };
    if let Some(manifest) = &release {
        manifest.check_wasm(&wasm).map_err(|e| {
            format!(
                "release {} does not name the Wasm in {wasm_path}: {e:?}",
                manifest.version
            )
        })?;
    }
    let operation = common.operation(OperationKind::InstallCode(InstallCode {
        mode,
        wasm_sha256: ByteBuf::from(sha256(&wasm)),
        chunk_hashes: release::chunks(&wasm)
            .map(|chunk| ByteBuf::from(sha256(chunk)))
            .collect(),
        arg: ByteBuf::from(arg),
        release,
    }));
    print_operation(operation)
}

fn set_policy(mut options: Options) -> Result<(), Failure> {
    let common = Common::read(&mut options)?;
    let policy = read_policy_file(&options.one("--policy")?)?;
    let proofs = read_proofs(&mut options)?;
    options.done(0)?;
    for proof in &proofs {
        let message = recovery::enroll_message(common.controller, common.wallet, &proof.public_key);
        if !recovery::verify(&proof.public_key, &message, &proof.signature) {
            return Err(Failure::Refused(format!(
                "the proof for key {} does not verify for this controller and wallet",
                hex(&proof.public_key)
            )));
        }
    }
    print_operation(common.operation(OperationKind::SetPolicy { policy, proofs }))
}

fn replace(mut options: Options) -> Result<(), Failure> {
    let common = Common::read(&mut options)?;
    let controller = principal_option(&mut options, "--new-controller")?;
    let module_hash = hash(&options.one("--module-hash")?)?;
    let policy = read_policy_file(&options.one("--policy")?)?;
    options.done(0)?;
    print_operation(common.operation(OperationKind::ReplaceController {
        controller,
        module_hash: ByteBuf::from(module_hash),
        policy_sha256: ByteBuf::from(policy.sha256()),
    }))
}

fn print_operation(operation: Operation) -> Result<(), Failure> {
    operation.check_form()?;
    eprint!("{}", describe(&operation));
    print!(
        "{}",
        Proposal {
            operation,
            approvals: vec![],
        }
        .text()
    );
    Ok(())
}

fn show(mut options: Options) -> Result<(), Failure> {
    let interface = options.optional("--interface")?;
    let [path] = options.done(1)?.try_into().expect("one argument");
    let proposal = read_proposal(&path)?;
    print!("{}", describe(&proposal.operation));
    if let (Some(interface), OperationKind::InstallCode(install)) =
        (interface, &proposal.operation.kind)
    {
        let (env, types) = init_types(&interface)?;
        let text = IDLArgs::from_bytes(&install.arg)
            .and_then(|args| args.annotate_types(true, &env, &types))
            .map_err(|e| {
                format!("the install argument is not the init argument of {interface}: {e}")
            })?;
        println!("Install argument: {text}");
    }
    match proposal.approvals.len() {
        0 => println!("No approvals in this file yet."),
        count => {
            println!("{count} approvals in this file, each verified against the digest:");
            for signed in &proposal.approvals {
                println!(
                    "  key {} ({})",
                    hex(&signed.public_key),
                    recovery::principal_of(&signed.public_key)
                        .map(|principal| principal.to_text())
                        .unwrap_or_default()
                );
            }
        }
    }
    Ok(())
}

fn sign(mut options: Options) -> Result<(), Failure> {
    let key = read_key(&read_text(&options.one("--key")?)?)?;
    let [path] = options.done(1)?.try_into().expect("one argument");
    let mut proposal = read_proposal(&path)?;
    eprint!("{}", describe(&proposal.operation));
    proposal.sign(&key);
    // A new file renamed over the old one, so a failed write leaves the
    // operation file as it was.
    let new = format!("{path}.new");
    std::fs::write(&new, proposal.text()).map_err(|e| format!("{new}: {e}"))?;
    std::fs::rename(&new, &path).map_err(|e| format!("{path}: {e}"))?;
    eprintln!(
        "Approved by {}. Approvals in the file: {}.",
        hex(key.verifying_key().as_bytes()),
        proposal.approvals.len()
    );
    Ok(())
}

fn approval(options: Options) -> Result<(), Failure> {
    let [path] = options.done(1)?.try_into().expect("one argument");
    let proposal = read_proposal(&path)?;
    print!("{}", proposal.approval_text());
    Ok(())
}

fn cancel(mut options: Options) -> Result<(), Failure> {
    let key = read_key(&read_text(&options.one("--key")?)?)?;
    let [path] = options.done(1)?.try_into().expect("one argument");
    let proposal = read_proposal(&path)?;
    print!("{}", cancel_text(&key, &proposal.operation));
    eprintln!(
        "Cancels operation {} with key {}.",
        proposal.operation.nonce,
        hex(key.verifying_key().as_bytes())
    );
    Ok(())
}

fn write_chunks(mut options: Options) -> Result<(), Failure> {
    let wasm_path = options.one("--wasm")?;
    let wasm = read(&wasm_path)?;
    let out = options.one("--out")?;
    let [path] = options.done(1)?.try_into().expect("one argument");
    let proposal = read_proposal(&path)?;
    let OperationKind::InstallCode(install) = &proposal.operation.kind else {
        return Err(Failure::Refused(format!(
            "operation {} installs no code",
            proposal.operation.nonce
        )));
    };
    let chunk_hashes: Vec<ByteBuf> = release::chunks(&wasm)
        .map(|chunk| ByteBuf::from(sha256(chunk)))
        .collect();
    if sha256(&wasm) != install.wasm_sha256.as_slice() || chunk_hashes != install.chunk_hashes {
        return Err(Failure::Refused(format!(
            "the Wasm in {wasm_path} (SHA-256 {}) is not the module operation {} installs",
            hex(&sha256(&wasm)),
            proposal.operation.nonce
        )));
    }
    std::fs::create_dir_all(&out).map_err(|e| format!("{out}: {e}"))?;
    let args = chunk_args(proposal.operation.nonce, &wasm);
    for (index, bytes) in args.iter().enumerate() {
        let path = Path::new(&out).join(format!("chunk-{index}.bin"));
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    eprintln!("Wrote {} chunks to {out}.", args.len());
    Ok(())
}

fn check(mut options: Options) -> Result<(), Failure> {
    let controller = principal_option(&mut options, "--controller")?;
    let wallet = principal_option(&mut options, "--wallet")?;
    let module_hash = hash(&options.one("--module-hash")?)?;
    let policy = read_policy_file(&options.one("--policy")?)?;
    let controller_status = read_status(&read_text(&options.one("--controller-status")?)?)?;
    let wallet_status = read_status(&read_text(&options.one("--wallet-status")?)?)?;
    let state = read_state(&read_text(&options.one("--state")?)?)?;
    let health = read_health(&read_text(&options.one("--health")?)?)?;
    let wallet_subnet = match options.optional("--wallet-subnet")? {
        Some(text) => Some(
            Principal::from_text(&text)
                .map_err(|e| Failure::Usage(format!("--wallet-subnet {text}: {e}")))?,
        ),
        None => None,
    };
    let before_hand_over = options.flag("--before-hand-over");
    options.done(0)?;
    let mut problems = Vec::new();
    if controller_status.module_hash.as_deref() != Some(&module_hash[..]) {
        problems.push(format!(
            "The controller runs {}, not the module {}.",
            controller_status
                .module_hash
                .as_deref()
                .map(hex)
                .unwrap_or_else(|| "no module".to_string()),
            hex(&module_hash)
        ));
    }
    if !controller_status.controllers.is_empty() {
        problems.push(format!(
            "The controller has controllers, who can change it: {}.",
            texts(&controller_status.controllers)
        ));
    }
    let expected = if before_hand_over { wallet } else { controller };
    if wallet_status.controllers != [expected] {
        problems.push(format!(
            "The wallet's controllers are [{}], not [{expected}].",
            texts(&wallet_status.controllers)
        ));
    }
    if state.wallet != wallet {
        problems.push(format!(
            "The controller serves wallet {}, not {wallet}.",
            state.wallet
        ));
    }
    if state.policy != policy || state.policy_sha256.as_slice() != policy.sha256() {
        problems.push(format!(
            "The controller holds another policy:\n{}",
            describe_policy(&state.policy)
        ));
    }
    if let Some(next) = state.replaced_by {
        problems.push(format!(
            "The controller handed the wallet to {next} and acts no more."
        ));
    }
    if state.wallet_stopped {
        problems.push(
            "The controller left the wallet stopped: start it with start_wallet.".to_string(),
        );
    }
    if before_hand_over && (state.policy_version != 1 || state.next_nonce != 1) {
        problems.push(
            "The controller has changed its policy or taken operations; the wallet hands itself \
             only to a controller as its install left it."
                .to_string(),
        );
    }
    if let Err(reason) = health.check(wallet_subnet.unwrap_or(health.subnet)) {
        problems.push(format!("The controller's settings: {reason}."));
    }
    println!(
        "Controller {controller}: module {}, controllers [{}].",
        controller_status
            .module_hash
            .as_deref()
            .map(hex)
            .unwrap_or_else(|| "none".to_string()),
        texts(&controller_status.controllers)
    );
    println!(
        "Wallet {wallet}: module {}, controllers [{}].",
        wallet_status
            .module_hash
            .as_deref()
            .map(hex)
            .unwrap_or_else(|| "none".to_string()),
        texts(&wallet_status.controllers)
    );
    println!(
        "The controller runs on subnet {}{}, with a freezing threshold of {} days.",
        health.subnet,
        if wallet_subnet.is_some() {
            ""
        } else {
            " (the wallet checks it is its own when it hands itself over)"
        },
        health.freezing_threshold.clone() / candid::Nat::from(86_400u32)
    );
    println!(
        "The controller's state: policy version {}, next nonce {}, {} cycles{}.",
        state.policy_version,
        state.next_nonce,
        state.cycles,
        match &state.execution {
            Some(execution) => format!(
                ", an execution at step {:?} since {}",
                execution.step,
                utc(execution.since_ns)
            ),
            None => String::new(),
        }
    );
    if problems.is_empty() {
        println!("Everything matches.");
        if before_hand_over {
            println!(
                "Propose the hand-over on the wallet's Recovery card with:\n  Controller id     \
                 {controller}\n  Its module hash   {}\n  Its policy hash   {}",
                hex(&module_hash),
                hex(&policy.sha256())
            );
        }
        Ok(())
    } else {
        Err(Failure::Refused(problems.join("\n")))
    }
}

fn texts(principals: &[Principal]) -> String {
    principals
        .iter()
        .map(Principal::to_text)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The install argument: `--arg FILE` of Candid bytes, or `--arg-candid
/// TEXT` with `--interface DID`.
fn install_arg(options: &mut Options) -> Result<Vec<u8>, Failure> {
    match (
        options.optional("--arg")?,
        options.optional("--arg-candid")?,
        options.optional("--interface")?,
    ) {
        (Some(path), None, None) => Ok(read(&path)?),
        (None, Some(text), Some(interface)) => {
            let (env, types) = init_types(&interface)?;
            let args = candid_parser::parse_idl_args(&text)
                .map_err(|e| format!("--arg-candid is not Candid text: {e}"))?;
            Ok(args.to_bytes_with_types(&env, &types).map_err(|e| {
                format!("--arg-candid is not the init argument of {interface}: {e}")
            })?)
        }
        _ => Err(Failure::Usage(
            "give the install argument as --arg FILE, or as --arg-candid TEXT with --interface \
             DID"
            .to_string(),
        )),
    }
}

/// The init argument types of the service in a `.did` file.
fn init_types(path: &str) -> Result<(TypeEnv, Vec<Type>), String> {
    let (env, actor) = CandidSource::File(Path::new(path))
        .load()
        .map_err(|e| format!("{path}: {e}"))?;
    let actor = actor.ok_or_else(|| format!("{path} has no service"))?;
    let class = env.trace_type(&actor).map_err(|e| format!("{path}: {e}"))?;
    match class.as_ref() {
        TypeInner::Class(types, _) => Ok((env, types.clone())),
        _ => Err(format!("the service in {path} takes no init argument")),
    }
}

fn read_proofs(options: &mut Options) -> Result<Vec<Proof>, Failure> {
    options
        .all("--proof")
        .iter()
        .map(|path| Ok(read_proof(&read_text(path)?).map_err(|e| format!("{path}: {e}"))?))
        .collect()
}

fn read_proposal(path: &str) -> Result<Proposal, String> {
    Proposal::read(&read_text(path)?).map_err(|e| format!("{path}: {e}"))
}

fn read_policy_file(path: &str) -> Result<Policy, String> {
    read_policy(&read_text(path)?).map_err(|e| format!("{path}: {e}"))
}

fn read_manifest(path: &str) -> Result<ReleaseManifest, String> {
    let manifest =
        release_tool::from_text(&read_text(path)?).map_err(|e| format!("{path}: {e}"))?;
    manifest.check_form().map_err(|e| format!("{path}: {e}"))?;
    Ok(manifest)
}

fn principal_option(options: &mut Options, name: &str) -> Result<Principal, Failure> {
    let text = options.one(name)?;
    Principal::from_text(&text).map_err(|e| Failure::Usage(format!("{name} {text}: {e}")))
}

fn hash(text: &str) -> Result<[u8; 32], String> {
    from_hex(text)?
        .try_into()
        .map_err(|_| format!("{text} is not a 32-byte hash"))
}

fn number<T: std::str::FromStr>(text: &str, name: &str) -> Result<T, Failure> {
    text.parse()
        .map_err(|_| Failure::Usage(format!("{name} {text} is not a number in range")))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_nanos() as u64
}

fn read(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{path}: {e}"))
}

fn read_text(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
}

/// `--name value` options, some repeated, `--name` flags, and arguments.
struct Options {
    values: HashMap<String, Vec<String>>,
    flags: Vec<String>,
    arguments: Vec<String>,
}

const FLAGS: [&str; 3] = ["--skip-pre-upgrade", "--reinstall", "--before-hand-over"];

impl Options {
    fn parse(args: &[String]) -> Result<Options, Failure> {
        let mut options = Options {
            values: HashMap::new(),
            flags: Vec::new(),
            arguments: Vec::new(),
        };
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            if FLAGS.contains(&arg.as_str()) {
                options.flags.push(arg.clone());
            } else if arg.starts_with("--") {
                let value = args
                    .next()
                    .ok_or_else(|| Failure::Usage(format!("{arg} needs a value")))?;
                options
                    .values
                    .entry(arg.clone())
                    .or_default()
                    .push(value.clone());
            } else {
                options.arguments.push(arg.clone());
            }
        }
        Ok(options)
    }

    fn all(&mut self, name: &str) -> Vec<String> {
        self.values.remove(name).unwrap_or_default()
    }

    fn optional(&mut self, name: &str) -> Result<Option<String>, Failure> {
        let mut values = self.all(name);
        match values.len() {
            0 => Ok(None),
            1 => Ok(values.pop()),
            _ => Err(Failure::Usage(format!("{name} given more than once"))),
        }
    }

    fn one(&mut self, name: &str) -> Result<String, Failure> {
        self.optional(name)?
            .ok_or_else(|| Failure::Usage(format!("{name} is needed")))
    }

    fn flag(&mut self, name: &str) -> bool {
        let given = self.flags.iter().any(|flag| flag == name);
        self.flags.retain(|flag| flag != name);
        given
    }

    /// Refuses options the command did not read, and returns its `count`
    /// arguments.
    fn done(self, count: usize) -> Result<Vec<String>, Failure> {
        if let Some(name) = self.values.keys().chain(&self.flags).next() {
            return Err(Failure::Usage(format!("unknown option {name}")));
        }
        if self.arguments.len() != count {
            return Err(Failure::Usage(format!(
                "expected {count} arguments, got {}",
                self.arguments.len()
            )));
        }
        Ok(self.arguments)
    }
}
