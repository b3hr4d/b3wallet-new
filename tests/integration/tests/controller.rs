//! Decision 0009: a real wallet handed to the real recovery controller, and
//! everything the owners' recovery keys can and cannot do with it, against
//! broken and hostile wallet code. Build the Wasm first:
//! `cargo build -p wallet -p controller -p caller --target wasm32-unknown-unknown --release`.
//! PocketIC uses the server binary named by `POCKET_IC_BIN`.
//!
//! Recovery keys are Ed25519 keys; PocketIC takes the sender of a call as
//! given, so a call "by key 11" is a call from that key's self-authenticating
//! principal. `tools/recovery-tool`'s own test signs real ingress messages.

use candid::{CandidType, Deserialize, Nat, Principal, decode_one, encode_args, encode_one};
use ed25519_dalek::{Signer, SigningKey};
use pocket_ic::{CreateCanisterParams, PocketIc, PocketIcBuilder, RejectResponse};
use recovery::controller::{
    ControllerArg, ControllerError, ControllerInit, ControllerState, Health, OperationStatus,
    OperationView, UploadChunk,
};
use recovery::{
    Authority, DAY_NS, HOUR_NS, InstallCode, InstallMode, Operation, OperationKind, Policy, Proof,
    Route, Signed,
};
use release::{CHUNK_SIZE, ReleaseManifest, chunks, sha256};
use serde_bytes::ByteBuf;
use std::time::Duration;
use wallet::{
    Action, ActionId, ActionState, ClaimArgs, InitArgs, Member, Outcome, Pending, Role, Status,
    WalletArg, WalletError,
};

fn read_wasm(variable: &str, name: &str) -> Vec<u8> {
    let path = std::env::var(variable).unwrap_or_else(|_| {
        format!(
            "{}/../../target/wasm32-unknown-unknown/release/{name}.wasm",
            env!("CARGO_MANIFEST_DIR")
        )
    });
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!("cannot read {path} ({e}); build it with `cargo build -p {name} --target wasm32-unknown-unknown --release`")
    })
}

fn wallet_wasm() -> Vec<u8> {
    read_wasm("WALLET_WASM", "wallet")
}

fn controller_wasm() -> Vec<u8> {
    read_wasm("CONTROLLER_WASM", "controller")
}

/// A canister that forwards any call: the wallet's code once it is broken
/// or hostile.
fn caller_wasm() -> Vec<u8> {
    read_wasm("CALLER_WASM", "caller")
}

fn leb128(mut n: usize, out: &mut Vec<u8>) {
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// `wasm` with a custom section that names `release`: another module with
/// the same code.
fn release_of(wasm: &[u8], release: &str) -> Vec<u8> {
    let name = b"b3wallet-test-release";
    let mut content = Vec::new();
    leb128(name.len(), &mut content);
    content.extend_from_slice(name);
    content.extend_from_slice(release.as_bytes());
    let mut module = wasm.to_vec();
    module.push(0);
    leb128(content.len(), &mut module);
    module.extend(content);
    module
}

/// A module whose `post_upgrade` traps, and nothing else.
fn trapping_module() -> Vec<u8> {
    let mut module = b"\0asm\x01\0\0\0".to_vec();
    module.extend([0x01, 0x04, 0x01, 0x60, 0x00, 0x00]);
    module.extend([0x03, 0x02, 0x01, 0x00]);
    module.extend([0x07, 0x19, 0x01, 0x15]);
    module.extend(b"canister_post_upgrade");
    module.extend([0x00, 0x00]);
    module.extend([0x0a, 0x05, 0x01, 0x03, 0x00, 0x00, 0x0b]);
    module
}

fn user(seed: u8) -> Principal {
    Principal::self_authenticating([seed; 32])
}

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn public(seed: u8) -> ByteBuf {
    ByteBuf::from(key(seed).verifying_key().to_bytes())
}

/// The principal a recovery key calls as.
fn by(seed: u8) -> Principal {
    recovery::principal_of(&public(seed)).unwrap()
}

/// The owner's three recovery keys, and the release signers.
const KEYS: [u8; 3] = [11, 12, 13];
const SIGNERS: [u8; 3] = [21, 22, 23];

/// Behrad's defaults: one key and 48 hours for a signed release, two keys
/// and 7 days for anything else.
fn policy_of(keys: &[u8], release_signers: bool) -> Policy {
    Policy {
        authorities: keys
            .iter()
            .map(|&seed| Authority {
                public_key: public(seed),
                label: format!("key {seed}"),
            })
            .collect(),
        normal_threshold: 1,
        sovereign_threshold: 2,
        normal_delay_ns: 48 * HOUR_NS,
        sovereign_delay_ns: 7 * DAY_NS,
        release_signers: if release_signers {
            SIGNERS.iter().map(|&seed| public(seed)).collect()
        } else {
            vec![]
        },
        release_threshold: if release_signers { 2 } else { 0 },
    }
}

fn proofs(controller: Principal, wallet: Principal, keys: &[u8]) -> Vec<Proof> {
    keys.iter()
        .map(|&seed| Proof {
            public_key: public(seed),
            signature: ByteBuf::from(
                key(seed)
                    .sign(&recovery::enroll_message(controller, wallet, &public(seed)))
                    .to_bytes(),
            ),
        })
        .collect()
}

fn approval(seed: u8, operation: &Operation) -> Signed {
    Signed {
        public_key: public(seed),
        signature: ByteBuf::from(
            key(seed)
                .sign(&recovery::approve_message(&operation.digest()))
                .to_bytes(),
        ),
    }
}

fn cancellation(seed: u8, operation: &Operation) -> Signed {
    Signed {
        public_key: public(seed),
        signature: ByteBuf::from(
            key(seed)
                .sign(&recovery::cancel_message(&operation.digest()))
                .to_bytes(),
        ),
    }
}

fn upgrade_arg() -> ByteBuf {
    ByteBuf::from(encode_one(WalletArg::Upgrade).unwrap())
}

fn no_arg() -> ByteBuf {
    ByteBuf::from(encode_args(()).unwrap())
}

fn chunk_hashes(wasm: &[u8]) -> Vec<ByteBuf> {
    chunks(wasm)
        .map(|chunk| ByteBuf::from(sha256(chunk)))
        .collect()
}

fn decode<R: CandidType + for<'de> Deserialize<'de>>(method: &str, reply: Vec<u8>) -> R {
    decode_one(&reply).unwrap_or_else(|e| panic!("{method}'s answer does not decode: {e}"))
}

/// A canister holding exactly `cycles`, controlled by the anonymous
/// principal.
fn canister_with(pic: &PocketIc, cycles: u128) -> Principal {
    pic.create_canister_with_params(
        None,
        CreateCanisterParams {
            cycles: Some(cycles),
            ..CreateCanisterParams::default()
        },
    )
    .unwrap()
}

/// A claimed wallet holding `cycles`, its own only controller, as the
/// factory leaves it.
fn claimed_wallet(pic: &PocketIc, cycles: u128) -> (Principal, Principal) {
    let channel = pic.create_canister();
    let wallet = canister_with(pic, cycles);
    let claim_key = SigningKey::from_bytes(&[1; 32]);
    let args = InitArgs {
        claim_public_key: ByteBuf::from(claim_key.verifying_key().to_bytes()),
        claim_expires_at_ns: pic.get_time().as_nanos_since_unix_epoch() + DAY_NS,
        release_channel: channel,
        release_signers: SIGNERS.iter().map(|&seed| public(seed)).collect(),
        release_threshold: 2,
        factory: None,
    };
    pic.install_canister(
        wallet,
        wallet_wasm(),
        encode_one(WalletArg::Init(args)).unwrap(),
        None,
    );
    pic.set_controllers(wallet, None, vec![wallet]).unwrap();
    let owner = user(7);
    let signature = ByteBuf::from(
        claim_key
            .sign(wallet::claim_message(wallet, owner).as_bytes())
            .to_bytes(),
    );
    let reply = pic
        .update_call(
            wallet,
            owner,
            "claim",
            encode_one(ClaimArgs { signature }).unwrap(),
        )
        .unwrap();
    decode::<Result<Member, WalletError>>("claim", reply).unwrap();
    (wallet, owner)
}

/// A controller for `wallet` with `policy`, installed with a proof of
/// possession for every key, with every controller removed.
fn controller_for(pic: &PocketIc, wallet: Principal, policy: Policy, cycles: u128) -> Principal {
    let controller = installed_controller(pic, wallet, policy, cycles);
    pic.set_controllers(controller, None, vec![]).unwrap();
    controller
}

/// Raises `canister`'s freezing threshold until it can spend only `keep`
/// cycles: the reserve it keeps to stay running grows with the threshold.
fn spendable_only(pic: &PocketIc, canister: Principal, sender: Option<Principal>, keep: u128) {
    let status = pic.canister_status(canister, sender).unwrap();
    let balance = u128::try_from(status.cycles.0).unwrap();
    let per_day = u128::try_from(status.idle_cycles_burned_per_day.0).unwrap();
    let seconds = (balance - keep) * 86_400 / per_day;
    pic.update_canister_settings(
        canister,
        sender,
        pocket_ic::CanisterSettings {
            freezing_threshold: Some(Nat::from(seconds)),
            ..pocket_ic::CanisterSettings::default()
        },
    )
    .unwrap();
}

/// `controller_for`, still controlled by the anonymous principal.
fn installed_controller(
    pic: &PocketIc,
    wallet: Principal,
    policy: Policy,
    cycles: u128,
) -> Principal {
    let controller = canister_with(pic, cycles);
    install_controller(pic, controller, wallet, policy);
    controller
}

/// Installs the controller in `controller`, an empty canister the anonymous
/// principal controls.
fn install_controller(pic: &PocketIc, controller: Principal, wallet: Principal, policy: Policy) {
    let keys: Vec<u8> = policy
        .authorities
        .iter()
        .map(|authority| {
            KEYS.into_iter()
                .chain([14, 15])
                .find(|&seed| public(seed) == authority.public_key)
                .unwrap()
        })
        .collect();
    let init = ControllerInit {
        wallet,
        proofs: proofs(controller, wallet, &keys),
        policy,
    };
    pic.install_canister(
        controller,
        controller_wasm(),
        encode_one(ControllerArg::Init(init)).unwrap(),
        None,
    );
}

/// A canister running the forwarding code, so a test can call as a
/// canister does.
fn caller_canister(pic: &PocketIc) -> Principal {
    let canister = canister_with(pic, 10_000_000_000_000);
    pic.install_canister(canister, caller_wasm(), encode_args(()).unwrap(), None);
    canister
}

/// Calls `method` of `canister` from `from`, a canister running the
/// forwarding code.
fn forward(
    pic: &PocketIc,
    from: Principal,
    canister: Principal,
    method: &str,
    arg: Vec<u8>,
) -> Result<Vec<u8>, String> {
    let reply = pic
        .update_call(
            from,
            user(99),
            "forward",
            encode_args((canister, method, ByteBuf::from(arg))).unwrap(),
        )
        .expect("forward answers");
    decode::<Result<ByteBuf, (u32, String)>>("forward", reply)
        .map(ByteBuf::into_vec)
        .map_err(|(code, message)| format!("{code}: {message}"))
}

struct Net {
    pic: PocketIc,
    wallet: Principal,
    controller: Principal,
    owner: Principal,
}

/// A claimed wallet handed to its controller through its own
/// `HandOverControl` action, with the default policy and release signers.
fn net() -> Net {
    net_with(PocketIc::new())
}

/// `net` on `pic`.
fn net_with(pic: PocketIc) -> Net {
    let (wallet, owner) = claimed_wallet(&pic, 20_000_000_000_000);
    let controller = installed_controller(&pic, wallet, policy_of(&KEYS, true), 10_000_000_000_000);
    pic.set_controllers(controller, None, vec![]).unwrap();
    let net = Net {
        pic,
        wallet,
        controller,
        owner,
    };
    let handed = net.hand_over(controller, sha256(&controller_wasm()), net.policy_sha256());
    assert_eq!(
        handed.state,
        ActionState::Executed(Outcome::Done),
        "{handed:?}"
    );
    assert_eq!(net.wallet_controllers(), vec![controller]);
    net
}

impl Net {
    fn now(&self) -> u64 {
        self.pic.get_time().as_nanos_since_unix_epoch()
    }

    fn wait(&self, ns: u64) {
        self.pic.advance_time(Duration::from_nanos(ns));
        self.pic.tick();
    }

    fn wallet_call<R: CandidType + for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        arg: Vec<u8>,
    ) -> R {
        let reply = self
            .pic
            .update_call(self.wallet, self.owner, method, arg)
            .unwrap_or_else(|reject| panic!("{method} answers: {reject}"));
        decode(method, reply)
    }

    fn propose_action(&self, action: Action) -> Result<Pending, WalletError> {
        self.wallet_call("propose", encode_one(action).unwrap())
    }

    fn execute_action(&self, id: ActionId) -> Result<Pending, WalletError> {
        self.wallet_call("execute", encode_one(id).unwrap())
    }

    /// Proposes a `HandOverControl`, waits out the `SetPolicy` rule's 7 days
    /// and executes it.
    fn hand_over(&self, controller: Principal, module: [u8; 32], policy: [u8; 32]) -> Pending {
        let proposed = self
            .propose_action(Action::HandOverControl {
                controller,
                controller_module_hash: ByteBuf::from(module.to_vec()),
                controller_policy_sha256: ByteBuf::from(policy.to_vec()),
            })
            .unwrap();
        assert!(matches!(
            self.execute_action(proposed.id),
            Err(WalletError::TimelockActive { .. })
        ));
        self.wait(7 * DAY_NS + 1);
        self.execute_action(proposed.id).unwrap()
    }

    fn status(&self) -> Result<Status, RejectResponse> {
        self.pic
            .query_call(
                self.wallet,
                Principal::anonymous(),
                "status",
                encode_one(()).unwrap(),
            )
            .map(|reply| decode("status", reply))
    }

    fn members(&self) -> Vec<Member> {
        let reply = self
            .pic
            .query_call(self.wallet, self.owner, "members", encode_one(()).unwrap())
            .unwrap();
        decode::<Result<Vec<Member>, WalletError>>("members", reply).unwrap()
    }

    fn wallet_controllers(&self) -> Vec<Principal> {
        self.pic.get_controllers(self.wallet)
    }

    fn wallet_module(&self) -> Vec<u8> {
        self.pic
            .canister_status(self.wallet, Some(self.controller))
            .unwrap()
            .module_hash
            .unwrap()
    }

    fn wallet_running(&self) -> bool {
        let status = self
            .pic
            .canister_status(self.wallet, Some(self.controller))
            .unwrap()
            .status;
        format!("{status:?}") == "Running"
    }

    fn try_controller(
        &self,
        caller: Principal,
        method: &str,
        arg: Vec<u8>,
    ) -> Result<Vec<u8>, RejectResponse> {
        self.pic.update_call(self.controller, caller, method, arg)
    }

    fn controller_call<R: CandidType + for<'de> Deserialize<'de>>(
        &self,
        caller: Principal,
        method: &str,
        arg: Vec<u8>,
    ) -> R {
        let reply = self
            .try_controller(caller, method, arg)
            .unwrap_or_else(|reject| panic!("{method} answers: {reject}"));
        decode(method, reply)
    }

    fn state(&self) -> ControllerState {
        let reply = self
            .pic
            .query_call(
                self.controller,
                Principal::anonymous(),
                "state",
                encode_one(()).unwrap(),
            )
            .unwrap();
        decode("state", reply)
    }

    fn policy_sha256(&self) -> [u8; 32] {
        self.state().policy_sha256.as_slice().try_into().unwrap()
    }

    fn operation(&self, id: u64) -> OperationView {
        let reply = self
            .pic
            .query_call(
                self.controller,
                Principal::anonymous(),
                "operation",
                encode_one(id).unwrap(),
            )
            .unwrap();
        decode::<Option<OperationView>>("operation", reply).unwrap()
    }

    /// An operation of `kind` with the controller's next nonce, under its
    /// current policy, expiring in 20 days.
    fn operation_of(&self, kind: OperationKind) -> Operation {
        let state = self.state();
        Operation {
            controller: self.controller,
            wallet: self.wallet,
            policy_version: state.policy_version,
            nonce: state.next_nonce,
            expires_at_ns: self.now() + 20 * DAY_NS,
            kind,
        }
    }

    fn install_of(
        &self,
        wasm: &[u8],
        mode: InstallMode,
        arg: ByteBuf,
        release: Option<ReleaseManifest>,
    ) -> Operation {
        self.operation_of(OperationKind::InstallCode(InstallCode {
            mode,
            wasm_sha256: ByteBuf::from(sha256(wasm)),
            chunk_hashes: chunk_hashes(wasm),
            arg,
            release,
        }))
    }

    fn propose(
        &self,
        caller: u8,
        operation: &Operation,
        approvers: &[u8],
    ) -> Result<OperationView, ControllerError> {
        let signed: Vec<Signed> = approvers
            .iter()
            .map(|&seed| approval(seed, operation))
            .collect();
        self.controller_call(
            by(caller),
            "propose",
            encode_args((operation, signed)).unwrap(),
        )
    }

    fn approve(
        &self,
        caller: u8,
        id: u64,
        signed: Vec<Signed>,
    ) -> Result<OperationView, ControllerError> {
        self.controller_call(by(caller), "approve", encode_args((id, signed)).unwrap())
    }

    fn cancel(
        &self,
        caller: u8,
        id: u64,
        signed: Signed,
    ) -> Result<OperationView, ControllerError> {
        self.controller_call(by(caller), "cancel", encode_args((id, signed)).unwrap())
    }

    fn upload(&self, caller: u8, id: u64, wasm: &[u8]) -> Result<Vec<u32>, ControllerError> {
        let mut missing = Err(ControllerError::NotFound("no chunk".to_string()));
        for (index, chunk) in chunks(wasm).enumerate() {
            missing = self.controller_call(
                by(caller),
                "upload_chunk",
                encode_one(UploadChunk {
                    operation: id,
                    index: index as u32,
                    chunk: ByteBuf::from(chunk.to_vec()),
                })
                .unwrap(),
            );
            missing.as_ref().map_err(|e| e.clone())?;
        }
        missing
    }

    fn execute(&self, caller: u8, id: u64) -> Result<OperationView, ControllerError> {
        self.controller_call(by(caller), "execute", encode_one(id).unwrap())
    }

    /// Proposes `operation` with the approvals of `approvers`, uploads its
    /// module if it installs one, and waits out its delay.
    fn ready(&self, operation: &Operation, approvers: &[u8], wasm: Option<&[u8]>) -> u64 {
        let proposed = self.propose(approvers[0], operation, approvers).unwrap();
        if let Some(wasm) = wasm {
            assert_eq!(self.upload(approvers[0], proposed.id, wasm), Ok(vec![]));
        }
        let Some(OperationStatus::Waiting { ready_at_ns }) = Some(proposed.status.clone()) else {
            panic!("not waiting: {proposed:?}");
        };
        assert!(matches!(
            self.execute(approvers[0], proposed.id),
            Err(ControllerError::TimelockActive { .. })
        ));
        self.wait(ready_at_ns - self.now());
        proposed.id
    }

    /// Executes an operation whose execution waits on a stop, moving the
    /// time on a minute a round until it answers.
    fn execute_slowly(&self, caller: u8, id: u64) -> Result<OperationView, ControllerError> {
        let message = self
            .pic
            .submit_call(
                self.controller,
                by(caller),
                "execute",
                encode_one(id).unwrap(),
            )
            .unwrap();
        for _ in 0..60 {
            self.pic.advance_time(Duration::from_secs(60));
            self.pic.tick();
            if let Some(reply) = self.pic.ingress_status(message.clone()) {
                return decode("execute", reply.expect("execute answers"));
            }
        }
        panic!("execute did not answer within an hour");
    }

    /// Has the wallet's code, the forwarding canister, give away every cycle
    /// it can spend but `keep`.
    fn drain(&self, keep: u128) {
        let sink = self.pic.create_canister();
        let reply = self
            .pic
            .update_call(
                self.wallet,
                user(99),
                "drain",
                encode_args((sink, keep)).unwrap(),
            )
            .unwrap();
        decode::<Result<(), String>>("drain", reply).unwrap();
    }

    /// The cycles the wallet can spend above its freezing reserve.
    fn wallet_liquid_cycles(&self) -> u128 {
        let status = self
            .pic
            .canister_status(self.wallet, Some(self.controller))
            .unwrap();
        let cycles = u128::try_from(status.cycles.0).unwrap();
        let per_day = u128::try_from(status.idle_cycles_burned_per_day.0).unwrap();
        let threshold = u128::try_from(status.settings.freezing_threshold.0).unwrap();
        cycles.saturating_sub(per_day * threshold / 86_400)
    }

    /// Calls `method` of `canister` as the wallet's code, which must be the
    /// forwarding canister.
    fn as_wallet(
        &self,
        canister: Principal,
        method: &str,
        arg: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        let reply = self
            .pic
            .update_call(
                self.wallet,
                user(99),
                "forward",
                encode_args((canister, method, ByteBuf::from(arg))).unwrap(),
            )
            .expect("forward answers");
        decode::<Result<ByteBuf, (u32, String)>>("forward", reply)
            .map(ByteBuf::into_vec)
            .map_err(|(code, message)| format!("{code}: {message}"))
    }
}

#[derive(CandidType)]
struct UploadChunkArgs {
    canister_id: Principal,
    chunk: ByteBuf,
}

#[derive(CandidType)]
struct CanisterIdRecord {
    canister_id: Principal,
}

#[derive(CandidType)]
struct Controllers {
    controllers: Option<Vec<Principal>>,
}

#[derive(CandidType)]
struct UpdateSettings {
    canister_id: Principal,
    settings: Controllers,
}

#[test]
fn a_wallet_hands_itself_to_a_verified_controller_and_keeps_running() {
    let net = net();
    let state = net.state();
    assert_eq!(state.wallet, net.wallet);
    assert_eq!(state.policy, policy_of(&KEYS, true));
    assert_eq!(state.policy_version, 1);
    assert_eq!(state.next_nonce, 1);
    assert_eq!(state.replaced_by, None);
    assert!(net.pic.get_controllers(net.controller).is_empty());
    assert_eq!(net.wallet_controllers(), vec![net.controller]);
    assert!(net.wallet_running());

    // The wallet runs as before: its members act, and nothing about it
    // changed but its controllers.
    let added = net
        .propose_action(Action::AddMember {
            id: user(8),
            role: Role::Viewer,
            label: "phone".to_string(),
        })
        .unwrap();
    assert_eq!(
        net.execute_action(added.id).unwrap().state,
        ActionState::Executed(Outcome::Done)
    );
    assert!(net.status().unwrap().claimed);

    // It can no longer install code itself, nor hand itself to anyone else.
    let again = net.hand_over(
        net.controller,
        sha256(&controller_wasm()),
        net.policy_sha256(),
    );
    assert_eq!(again.state, ActionState::Executed(Outcome::Done));
    let elsewhere = controller_for(
        &net.pic,
        net.wallet,
        policy_of(&KEYS, true),
        1_000_000_000_000,
    );
    let refused = net
        .propose_action(Action::HandOverControl {
            controller: elsewhere,
            controller_module_hash: ByteBuf::from(sha256(&controller_wasm()).to_vec()),
            controller_policy_sha256: ByteBuf::from(net.policy_sha256().to_vec()),
        })
        .unwrap();
    net.wait(7 * DAY_NS + 1);
    assert!(matches!(
        net.execute_action(refused.id),
        Err(WalletError::Forbidden(reason)) if reason.contains("not its own controller")
    ));
    assert_eq!(net.wallet_controllers(), vec![net.controller]);
}

#[test]
fn the_wallet_refuses_a_controller_it_cannot_verify() {
    let pic = PocketIc::new();
    let (wallet, owner) = claimed_wallet(&pic, 20_000_000_000_000);
    let module = sha256(&controller_wasm());
    let policy = policy_of(&KEYS, true);
    let net = Net {
        pic,
        wallet,
        controller: Principal::anonymous(),
        owner,
    };
    let failed = |controller: Principal, module: [u8; 32], policy: [u8; 32]| -> String {
        match net.hand_over(controller, module, policy).state {
            ActionState::Failed(reason) => reason,
            other => panic!("not failed: {other:?}"),
        }
    };

    let good = controller_for(&net.pic, wallet, policy.clone(), 1_000_000_000_000);
    assert!(failed(good, [9; 32], policy.sha256()).contains("not the approved"));
    assert!(failed(good, module, [9; 32]).contains("another policy"));

    // A controller someone still controls could be changed at will.
    let held = net.pic.create_canister();
    net.pic.add_cycles(held, 1_000_000_000_000);
    net.pic.install_canister(
        held,
        controller_wasm(),
        encode_one(ControllerArg::Init(ControllerInit {
            wallet,
            proofs: proofs(held, wallet, &KEYS),
            policy: policy.clone(),
        }))
        .unwrap(),
        None,
    );
    assert!(failed(held, module, policy.sha256()).contains("must have none"));

    // One for another wallet.
    let (other_wallet, _) = claimed_wallet(&net.pic, 1_000_000_000_000);
    let theirs = controller_for(&net.pic, other_wallet, policy.clone(), 1_000_000_000_000);
    assert!(failed(theirs, module, policy.sha256()).contains("not this one"));

    // One whose memory came from a snapshot instead of its own install,
    // and one installed twice.
    let restored = net.pic.create_canister();
    net.pic.add_cycles(restored, 1_000_000_000_000);
    net.pic.install_canister(
        restored,
        controller_wasm(),
        encode_one(ControllerArg::Init(ControllerInit {
            wallet,
            proofs: proofs(restored, wallet, &KEYS),
            policy: policy.clone(),
        }))
        .unwrap(),
        None,
    );
    net.pic.stop_canister(restored, None).unwrap();
    let snapshot = net
        .pic
        .take_canister_snapshot(restored, None, None)
        .unwrap();
    net.pic
        .load_canister_snapshot(restored, None, snapshot.id)
        .unwrap();
    net.pic.start_canister(restored, None).unwrap();
    net.pic.set_controllers(restored, None, vec![]).unwrap();
    assert!(failed(restored, module, policy.sha256()).contains("snapshot"));

    let twice = net.pic.create_canister();
    net.pic.add_cycles(twice, 1_000_000_000_000);
    let init = encode_one(ControllerArg::Init(ControllerInit {
        wallet,
        proofs: proofs(twice, wallet, &KEYS),
        policy: policy.clone(),
    }))
    .unwrap();
    net.pic
        .install_canister(twice, controller_wasm(), init.clone(), None);
    net.pic
        .reinstall_canister(twice, controller_wasm(), init, None)
        .unwrap();
    net.pic.set_controllers(twice, None, vec![]).unwrap();
    assert!(failed(twice, module, policy.sha256()).contains("other than by one install"));

    // One whose creator raised its freezing threshold past a year: nobody
    // can lower it once it has no controller, and the owners would have to
    // keep that much of its idle burn on it for it to act at all.
    let frozen = installed_controller(&net.pic, wallet, policy.clone(), 5_000_000_000_000);
    net.pic
        .update_canister_settings(
            frozen,
            None,
            pocket_ic::CanisterSettings {
                freezing_threshold: Some(Nat::from(400 * 86_400u64)),
                ..pocket_ic::CanisterSettings::default()
            },
        )
        .unwrap();
    net.pic.set_controllers(frozen, None, vec![]).unwrap();
    assert!(failed(frozen, module, policy.sha256()).contains("freezing threshold"));

    // One that took an operation before the hand-over: the wallet takes only
    // a controller as its install left it.
    let busy = controller_for(&net.pic, wallet, policy.clone(), 1_000_000_000_000);
    let early = Operation {
        controller: busy,
        wallet,
        policy_version: 1,
        nonce: 1,
        expires_at_ns: net.now() + 20 * DAY_NS,
        kind: OperationKind::InstallCode(InstallCode {
            mode: InstallMode::Upgrade {
                skip_pre_upgrade: false,
            },
            wasm_sha256: ByteBuf::from(sha256(&caller_wasm())),
            chunk_hashes: chunk_hashes(&caller_wasm()),
            arg: no_arg(),
            release: None,
        }),
    };
    let reply = net
        .pic
        .update_call(
            busy,
            by(11),
            "propose",
            encode_args((&early, vec![approval(11, &early), approval(12, &early)])).unwrap(),
        )
        .unwrap();
    decode::<Result<OperationView, ControllerError>>("propose", reply).unwrap();
    assert!(failed(busy, module, policy.sha256()).contains("taken operations"));

    // None of it changed who controls the wallet.
    assert_eq!(net.pic.get_controllers(wallet), vec![wallet]);
    // A proposal that is not a canister is refused before it is stored.
    assert!(matches!(
        net.propose_action(Action::HandOverControl {
            controller: user(3),
            controller_module_hash: ByteBuf::from(module.to_vec()),
            controller_policy_sha256: ByteBuf::from(policy.sha256().to_vec()),
        }),
        Err(WalletError::InvalidArgument(_))
    ));
}

#[test]
fn an_interrupted_hand_over_ends_where_it_stopped() {
    let pic = PocketIc::new();
    let (wallet, owner) = claimed_wallet(&pic, 20_000_000_000_000);
    let controller = controller_for(&pic, wallet, policy_of(&KEYS, true), 10_000_000_000_000);
    let net = Net {
        pic,
        wallet,
        controller,
        owner,
    };

    // Before the hand-over the controller has no power over the wallet: an
    // approved repair cannot even read its status.
    let repair = net.install_of(
        &release_of(&wallet_wasm(), "repair"),
        InstallMode::Upgrade {
            skip_pre_upgrade: false,
        },
        upgrade_arg(),
        None,
    );
    let id = net.ready(
        &repair,
        &[11, 12],
        Some(&release_of(&wallet_wasm(), "repair")),
    );
    assert!(matches!(
        net.execute(11, id),
        Err(ControllerError::ExecutionFailed(reason)) if reason.contains("cannot read the wallet's status")
    ));
    assert!(net.operation(id).status.is_open());
    assert_eq!(net.pic.get_controllers(wallet), vec![wallet]);

    // The hand-over ran but its answer was lost: the wallet's controllers
    // already are the controller when the action executes again.
    let proposed = net
        .propose_action(Action::HandOverControl {
            controller,
            controller_module_hash: ByteBuf::from(sha256(&controller_wasm()).to_vec()),
            controller_policy_sha256: ByteBuf::from(net.policy_sha256().to_vec()),
        })
        .unwrap();
    net.wait(7 * DAY_NS + 1);
    net.pic
        .set_controllers(wallet, Some(wallet), vec![controller])
        .unwrap();
    assert_eq!(
        net.execute_action(proposed.id).unwrap().state,
        ActionState::Executed(Outcome::Done)
    );

    // From here the controller acts.
    let executed = net.execute(11, id).unwrap();
    assert!(matches!(executed.status, OperationStatus::Executed { .. }));
    assert_eq!(
        net.wallet_module(),
        sha256(&release_of(&wallet_wasm(), "repair"))
    );
}

#[test]
fn a_signed_release_installs_on_the_release_route() {
    let net = net();
    let before = net.members();
    let current = net.wallet_module();
    let next = release_of(&wallet_wasm(), "0.2.0");
    let mut manifest = ReleaseManifest::new(
        "0.2.0".to_string(),
        &next,
        vec![current.clone().try_into().unwrap()],
        "https://example.org/releases/0.2.0".to_string(),
        net.now(),
    );
    manifest.sign(&key(21));

    // One signature is not the policy's two: the release route is closed.
    let one = net.install_of(
        &next,
        InstallMode::Upgrade {
            skip_pre_upgrade: false,
        },
        upgrade_arg(),
        Some(manifest.clone()),
    );
    assert!(matches!(
        net.propose(11, &one, &[11]),
        Err(ControllerError::InvalidArgument(reason)) if reason.contains("1 of the 2")
    ));
    manifest.sign(&key(22));
    let operation = net.install_of(
        &next,
        InstallMode::Upgrade {
            skip_pre_upgrade: false,
        },
        upgrade_arg(),
        Some(manifest.clone()),
    );
    let proposed = net.propose(11, &operation, &[11]).unwrap();
    assert_eq!(proposed.route, Route::Release);
    assert_eq!(proposed.threshold, 1);
    assert_eq!(proposed.delay_ns, 48 * HOUR_NS);

    // Chunks must hash to the approved ones.
    let mut wrong = chunks(&next).next().unwrap().to_vec();
    wrong[0] ^= 1;
    let refused: Result<Vec<u32>, ControllerError> = net.controller_call(
        by(11),
        "upload_chunk",
        encode_one(UploadChunk {
            operation: proposed.id,
            index: 0,
            chunk: ByteBuf::from(wrong),
        })
        .unwrap(),
    );
    assert!(matches!(refused, Err(ControllerError::InvalidArgument(_))));
    assert!(matches!(
        net.execute(11, proposed.id),
        Err(ControllerError::TimelockActive { .. })
    ));
    net.wait(48 * HOUR_NS);
    assert!(matches!(
        net.execute(11, proposed.id),
        Err(ControllerError::ChunksMissing(missing)) if missing.len() == manifest.chunk_hashes.len()
    ));
    assert_eq!(net.upload(12, proposed.id, &next), Ok(vec![]));

    let executed = net.execute(13, proposed.id).unwrap();
    assert!(matches!(executed.status, OperationStatus::Executed { .. }));
    assert_eq!(executed.attempts, 1);
    assert_eq!(net.wallet_module(), sha256(&next));
    assert!(net.wallet_running());
    assert_eq!(net.members(), before);
    assert!(net.status().unwrap().claimed);
    assert!(matches!(
        net.execute(11, proposed.id),
        Err(ControllerError::Closed(OperationStatus::Executed { .. }))
    ));

    // A release not tested from the module the wallet runs now is refused at
    // execution, and the wallet keeps running.
    let after = release_of(&wallet_wasm(), "0.3.0");
    let mut untested = ReleaseManifest::new(
        "0.3.0".to_string(),
        &after,
        vec![current.try_into().unwrap()],
        "https://example.org/releases/0.3.0".to_string(),
        net.now(),
    );
    untested.sign(&key(22));
    untested.sign(&key(23));
    let operation = net.install_of(
        &after,
        InstallMode::Upgrade {
            skip_pre_upgrade: false,
        },
        upgrade_arg(),
        Some(untested),
    );
    let id = net.ready(&operation, &[12], Some(&after));
    assert!(matches!(
        net.execute(12, id),
        Err(ControllerError::Forbidden(reason)) if reason.contains("sovereign route")
    ));
    assert_eq!(net.wallet_module(), sha256(&next));
    assert!(net.wallet_running());
}

#[test]
fn owners_repair_broken_and_hostile_wallet_code_without_any_release_service() {
    let net = net();
    let members = net.members();

    // Code that breaks the wallet's pages, its API, its authorization and
    // its upgrade method, installed on the sovereign route: two keys, 7 days.
    let broken = caller_wasm();
    let operation = net.install_of(
        &broken,
        InstallMode::Upgrade {
            skip_pre_upgrade: false,
        },
        no_arg(),
        None,
    );
    let proposed = net.propose(11, &operation, &[11]).unwrap();
    assert_eq!(proposed.route, Route::Sovereign);
    assert!(matches!(
        net.upload(11, proposed.id, &broken),
        Err(ControllerError::Forbidden(reason)) if reason.contains("2 keys approved")
    ));
    net.approve(12, proposed.id, vec![approval(12, &operation)])
        .unwrap();
    assert_eq!(net.upload(11, proposed.id, &broken), Ok(vec![]));
    net.wait(7 * DAY_NS);
    net.execute(12, proposed.id).unwrap();
    assert_eq!(net.wallet_module(), sha256(&broken));
    assert!(net.status().is_err(), "the wallet's API is gone");

    // The wallet's code cannot take control back, reach the controller's
    // privileged paths, or touch the controller's chunk store.
    let management = Principal::management_canister();
    let own = encode_one(CanisterIdRecord {
        canister_id: net.wallet,
    })
    .unwrap();
    let theirs = encode_one(CanisterIdRecord {
        canister_id: net.controller,
    })
    .unwrap();
    for (canister, method, arg) in [
        (
            management,
            "update_settings",
            encode_one(UpdateSettings {
                canister_id: net.wallet,
                settings: Controllers {
                    controllers: Some(vec![net.wallet]),
                },
            })
            .unwrap(),
        ),
        (management, "stop_canister", own.clone()),
        (management, "uninstall_code", own.clone()),
        (management, "take_canister_snapshot", own.clone()),
        (management, "canister_status", theirs.clone()),
        (management, "clear_chunk_store", theirs.clone()),
        (management, "stop_canister", theirs.clone()),
        (
            management,
            "update_settings",
            encode_one(UpdateSettings {
                canister_id: net.controller,
                settings: Controllers {
                    controllers: Some(vec![net.wallet]),
                },
            })
            .unwrap(),
        ),
        (
            net.controller,
            "propose",
            encode_args((&operation, Vec::<Signed>::new())).unwrap(),
        ),
        (net.controller, "execute", encode_one(1u64).unwrap()),
        (net.controller, "start_wallet", encode_args(()).unwrap()),
    ] {
        assert!(
            net.as_wallet(canister, method, arg).is_err(),
            "the wallet's code could call {method} of {canister}"
        );
    }
    assert_eq!(net.wallet_controllers(), vec![net.controller]);
    assert!(net.pic.get_controllers(net.controller).is_empty());
    // It can fill its own chunk store while it runs, with a chunk of the
    // module the owners will install among others. The controller empties
    // the store once the wallet is stopped, and stages the module there.
    let repaired = wallet_wasm();
    for chunk in [&b"junk"[..], &repaired[..CHUNK_SIZE], &[0; 1000]] {
        let arg = encode_one(UploadChunkArgs {
            canister_id: net.wallet,
            chunk: ByteBuf::from(chunk),
        })
        .unwrap();
        assert!(net.as_wallet(management, "upload_chunk", arg).is_ok());
    }

    // The release channel the wallet names is gone; the owners need neither
    // it nor a signed release to install the real wallet again.
    let repair = net.install_of(
        &repaired,
        InstallMode::Upgrade {
            skip_pre_upgrade: false,
        },
        upgrade_arg(),
        None,
    );
    let id = net.ready(&repair, &[12, 13], Some(&repaired));
    let executed = net.execute(13, id).unwrap();
    assert!(matches!(executed.status, OperationStatus::Executed { .. }));
    assert_eq!(net.wallet_module(), sha256(&repaired));
    let status = net.status().unwrap();
    assert!(status.claimed);
    assert_eq!(net.members(), members);
    assert!(net.wallet_running());
}

/// A wallet hands itself only to a controller on its own subnet, so the
/// controller adds no subnet the wallet depends on. The controller stages
/// modules in the wallet's own chunk store all the same, so one that got the
/// wallet across subnets some other way still installs its code.
#[test]
fn a_controller_on_another_subnet_installs_the_wallets_code() {
    let pic = PocketIcBuilder::new()
        .with_application_subnet()
        .with_application_subnet()
        .build();
    let (wallet, owner) = claimed_wallet(&pic, 20_000_000_000_000);
    let other = pic
        .topology()
        .get_app_subnets()
        .into_iter()
        .find(|&subnet| Some(subnet) != pic.get_subnet(wallet))
        .unwrap();
    let controller = pic.create_canister_on_subnet(None, None, other);
    install_controller(&pic, controller, wallet, policy_of(&KEYS, true));
    pic.set_controllers(controller, None, vec![]).unwrap();
    assert_ne!(pic.get_subnet(controller), pic.get_subnet(wallet));
    let net = Net {
        pic,
        wallet,
        controller,
        owner,
    };
    let refused = net.hand_over(controller, sha256(&controller_wasm()), net.policy_sha256());
    assert!(
        matches!(&refused.state, ActionState::Failed(reason) if reason.contains("not the wallet's subnet")),
        "{refused:?}"
    );
    assert_eq!(net.wallet_controllers(), vec![wallet]);
    net.pic
        .set_controllers(wallet, Some(wallet), vec![controller])
        .unwrap();

    let next = release_of(&wallet_wasm(), "9.9.9");
    let operation = net.install_of(
        &next,
        InstallMode::Upgrade {
            skip_pre_upgrade: false,
        },
        upgrade_arg(),
        None,
    );
    let id = net.ready(&operation, &[11, 12], Some(&next));
    let executed = net.execute(11, id).unwrap();
    assert!(matches!(executed.status, OperationStatus::Executed { .. }));
    assert_eq!(net.wallet_module(), sha256(&next));
    assert!(net.wallet_running());
    assert!(net.status().unwrap().claimed);
}

#[test]
fn a_release_whose_post_upgrade_traps_leaves_the_wallet_running_and_the_operation_open() {
    let net = net();
    let running = net.wallet_module();
    let trapping = trapping_module();
    let operation = net.install_of(
        &trapping,
        InstallMode::Upgrade {
            skip_pre_upgrade: false,
        },
        no_arg(),
        None,
    );
    let id = net.ready(&operation, &[11, 13], Some(&trapping));
    for attempt in 1..=2 {
        assert!(matches!(
            net.execute(11, id),
            Err(ControllerError::ExecutionFailed(reason)) if reason.contains("install_chunked_code")
        ));
        let view = net.operation(id);
        assert_eq!(view.status, OperationStatus::Ready);
        assert_eq!(view.attempts, attempt);
        assert!(view.last_error.is_some());
        assert_eq!(net.wallet_module(), running);
        assert!(net.wallet_running());
        assert!(!net.state().wallet_stopped);
        assert_eq!(net.state().execution, None);
        assert!(net.status().unwrap().claimed);
    }
    let cancelled = net.cancel(12, id, cancellation(12, &operation)).unwrap();
    assert!(matches!(
        cancelled.status,
        OperationStatus::Cancelled { .. }
    ));
    assert!(cancelled.missing_chunks.is_empty());
}

/// An install the IC rejected changed nothing, so the controller forgets it
/// sent it: when another operation then installs the same module, the
/// rejected one is not taken for having run.
#[test]
fn a_rejected_install_is_not_taken_for_one_that_ran_later() {
    let net = net();
    let next = release_of(&wallet_wasm(), "0.3.0");
    let upgrade = InstallMode::Upgrade {
        skip_pre_upgrade: false,
    };
    // The wallet's post_upgrade traps with no WalletArg.
    let rejected = net.install_of(&next, upgrade, no_arg(), None);
    let rejected_id = net.propose(11, &rejected, &[11, 12]).unwrap().id;
    assert_eq!(net.upload(11, rejected_id, &next), Ok(vec![]));
    let good = net.install_of(&next, upgrade, upgrade_arg(), None);
    let good_id = net.ready(&good, &[12, 13], Some(&next));
    let trapped = |result: Result<OperationView, ControllerError>| matches!(result, Err(ControllerError::ExecutionFailed(reason)) if reason.contains("install_chunked_code"));
    assert!(trapped(net.execute(11, rejected_id)));
    net.execute(12, good_id).unwrap();
    assert_eq!(net.wallet_module(), sha256(&next));
    // Executing it again sends its install again, which the IC rejects again.
    assert!(trapped(net.execute(11, rejected_id)));
    let view = net.operation(rejected_id);
    assert_eq!(view.status, OperationStatus::Ready);
    assert_eq!(view.attempts, 2);
    assert!(net.status().unwrap().claimed);
}

#[test]
fn operations_are_bound_to_one_controller_wallet_policy_nonce_and_window() {
    let net = net();
    let wasm = release_of(&wallet_wasm(), "0.2.0");
    let base = net.install_of(
        &wasm,
        InstallMode::Upgrade {
            skip_pre_upgrade: false,
        },
        upgrade_arg(),
        None,
    );
    let changed = |change: &dyn Fn(&mut Operation)| {
        let mut operation = base.clone();
        change(&mut operation);
        operation
    };
    for (operation, expected) in [
        (changed(&|op| op.controller = net.wallet), "for controller"),
        (changed(&|op| op.wallet = net.controller), "for wallet"),
        (changed(&|op| op.policy_version = 2), "policy version"),
        (changed(&|op| op.nonce = 2), "nonce"),
        (changed(&|op| op.nonce = 0), "nonce"),
        (
            changed(&|op| op.expires_at_ns = net.now()),
            "after its delay",
        ),
        // It could never run: it would expire before its 7 days pass.
        (
            changed(&|op| op.expires_at_ns = net.now() + 7 * DAY_NS),
            "after its delay",
        ),
        (
            changed(&|op| op.expires_at_ns = net.now() + 61 * DAY_NS),
            "60 days",
        ),
    ] {
        assert!(
            matches!(
                net.propose(11, &operation, &[11]),
                Err(ControllerError::InvalidArgument(reason)) if reason.contains(expected)
            ),
            "{expected}"
        );
    }

    // Approvals sign the digest of exactly this operation, by a key of this
    // policy, once.
    let altered = changed(&|op| op.expires_at_ns += 1);
    let signed_other = vec![approval(11, &altered)];
    assert!(matches!(
        net.controller_call::<Result<OperationView, ControllerError>>(
            by(11),
            "propose",
            encode_args((&base, signed_other)).unwrap()
        ),
        Err(ControllerError::BadSignature(_))
    ));
    assert!(matches!(
        net.propose(11, &base, &[14]),
        Err(ControllerError::BadSignature(_))
    ));
    let proposed = net.propose(11, &base, &[11]).unwrap();
    assert_eq!(proposed.digest.as_slice(), base.digest());
    assert!(matches!(
        net.approve(12, proposed.id, vec![approval(11, &base)]),
        Err(ControllerError::InvalidArgument(reason)) if reason.contains("approved this operation already")
    ));
    assert!(matches!(
        net.approve(12, proposed.id, vec![approval(12, &altered)]),
        Err(ControllerError::BadSignature(_))
    ));
    // The same operation again is a replay: its nonce is used.
    assert!(matches!(
        net.propose(12, &base, &[12]),
        Err(ControllerError::InvalidArgument(reason)) if reason.contains("nonce")
    ));
    assert!(matches!(
        net.execute(11, proposed.id),
        Err(ControllerError::QuorumNotMet {
            approvals: 1,
            threshold: 2
        })
    ));
    let approved = net
        .approve(13, proposed.id, vec![approval(13, &base)])
        .unwrap();
    assert!(matches!(approved.status, OperationStatus::Waiting { .. }));
    net.upload(11, proposed.id, &wasm).unwrap();
    // It expires before anyone executes it.
    net.wait(20 * DAY_NS);
    assert_eq!(net.operation(proposed.id).status, OperationStatus::Expired);
    assert!(matches!(
        net.execute(11, proposed.id),
        Err(ControllerError::Closed(OperationStatus::Expired))
    ));
    assert!(net.operation(proposed.id).missing_chunks.is_empty());

    // Strangers and the wallet's owner are no recovery keys: the controller
    // takes no message of theirs, a read sent as an update included, so they
    // cannot spend its cycles.
    let cycles = net.state().cycles;
    for stranger in [user(99), net.owner, Principal::anonymous(), net.wallet] {
        for method in [
            "propose",
            "approve",
            "cancel",
            "upload_chunk",
            "execute",
            "start_wallet",
            "state",
            "operation",
            "operations",
            "health",
        ] {
            let refused = net.try_controller(stranger, method, encode_args(()).unwrap());
            assert!(
                matches!(&refused, Err(reject) if reject.reject_message.contains("rejected the message")),
                "{stranger} could send {method}: {refused:?}"
            );
        }
    }
    assert_eq!(net.state().cycles, cycles);
    // Anyone may read with a query call, and a canister with a call, which it
    // pays for; a key may read with an update.
    assert_eq!(net.state().wallet, net.wallet);
    let reader = caller_canister(&net.pic);
    let state: ControllerState = decode(
        "state",
        forward(
            &net.pic,
            reader,
            net.controller,
            "state",
            encode_args(()).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(state.wallet, net.wallet);
    let health: Health = decode(
        "health",
        forward(
            &net.pic,
            reader,
            net.controller,
            "health",
            encode_args(()).unwrap(),
        )
        .unwrap(),
    );
    assert!(health.controllers.is_empty());
    assert_eq!(
        Some(health.subnet),
        net.pic.get_subnet(net.controller),
        "{health:?}"
    );
    let state: ControllerState = net.controller_call(by(13), "state", encode_args(()).unwrap());
    assert_eq!(state.wallet, net.wallet);
    // A canister cannot change anything.
    assert!(
        forward(
            &net.pic,
            reader,
            net.controller,
            "propose",
            encode_args((&base, Vec::<Signed>::new())).unwrap()
        )
        .is_err()
    );
}

/// Every operation stays stored, so one key, stolen or not, proposes at
/// most 8 a day, however many it cancels.
#[test]
fn one_key_proposes_a_bounded_number_of_operations_a_day() {
    let net = net();
    let wasm = caller_wasm();
    let propose_and_cancel = |caller: u8| {
        let operation = net.install_of(
            &wasm,
            InstallMode::Upgrade {
                skip_pre_upgrade: false,
            },
            no_arg(),
            None,
        );
        let proposed = net.propose(caller, &operation, &[])?;
        net.cancel(caller, proposed.id, cancellation(caller, &operation))
            .unwrap();
        Ok::<_, ControllerError>(proposed.id)
    };
    for _ in 0..8 {
        propose_and_cancel(11).unwrap();
    }
    assert!(matches!(
        propose_and_cancel(11),
        Err(ControllerError::Forbidden(reason)) if reason.contains("in the last 24 hours")
    ));
    // Other keys are not held back by it, and it may propose again a day on.
    propose_and_cancel(12).unwrap();
    net.wait(DAY_NS);
    propose_and_cancel(11).unwrap();
}

#[test]
fn thresholds_delays_cancellation_and_key_rotation() {
    let net = net();
    let wasm = release_of(&wallet_wasm(), "0.2.0");
    let install = net.install_of(
        &wasm,
        InstallMode::Upgrade {
            skip_pre_upgrade: false,
        },
        upgrade_arg(),
        None,
    );
    let install_id = net.propose(11, &install, &[11, 12]).unwrap().id;

    // Key 13 is lost: keys 11 and 12 rotate it out for key 14. The new key
    // proves possession; key 13 cannot cancel its own removal.
    let mut next = policy_of(&[11, 12, 14], true);
    next.authorities[2].label = "new paper key".to_string();
    let without_proof = net.operation_of(OperationKind::SetPolicy {
        policy: next.clone(),
        proofs: vec![],
    });
    assert!(matches!(
        net.propose(11, &without_proof, &[11]),
        Err(ControllerError::InvalidArgument(reason)) if reason.contains("proof of possession")
    ));
    let rotate = net.operation_of(OperationKind::SetPolicy {
        policy: next.clone(),
        proofs: proofs(net.controller, net.wallet, &[14]),
    });
    let rotation = net.propose(12, &rotate, &[12]).unwrap();
    assert_eq!(rotation.route, Route::Sovereign);
    assert!(matches!(
        net.cancel(13, rotation.id, cancellation(13, &rotate)),
        Err(ControllerError::Forbidden(_))
    ));
    // Any key cancels anything else, such as an install it does not want.
    let unwanted = net
        .cancel(13, install_id, cancellation(13, &install))
        .unwrap();
    assert!(matches!(unwanted.status, OperationStatus::Cancelled { .. }));
    assert!(matches!(
        net.approve(11, install_id, vec![approval(11, &install)]),
        Err(ControllerError::Closed(OperationStatus::Cancelled { .. }))
    ));

    // An install proposed meanwhile goes stale when the policy changes.
    let pending = net.install_of(
        &wasm,
        InstallMode::Upgrade {
            skip_pre_upgrade: false,
        },
        upgrade_arg(),
        None,
    );
    let pending_id = net.propose(13, &pending, &[13]).unwrap().id;

    net.approve(11, rotation.id, vec![approval(11, &rotate)])
        .unwrap();
    net.wait(7 * DAY_NS - HOUR_NS);
    assert!(matches!(
        net.execute(11, rotation.id),
        Err(ControllerError::TimelockActive { .. })
    ));
    net.wait(HOUR_NS);
    net.execute(11, rotation.id).unwrap();

    let state = net.state();
    assert_eq!(state.policy, next);
    assert_eq!(state.policy_version, 2);
    assert_eq!(net.operation(pending_id).status, OperationStatus::Stale);
    // Key 13 is out; key 14 is in.
    assert!(
        net.try_controller(by(13), "execute", encode_one(pending_id).unwrap())
            .is_err()
    );
    let repair = net.install_of(
        &wasm,
        InstallMode::Upgrade {
            skip_pre_upgrade: false,
        },
        upgrade_arg(),
        None,
    );
    assert_eq!(repair.policy_version, 2);
    let id = net.ready(&repair, &[14, 11], Some(&wasm));
    net.execute(14, id).unwrap();
    assert_eq!(net.wallet_module(), sha256(&wasm));
}

/// Wallet code can give away the wallet's cycles, so that it cannot pay for
/// the install that repairs it. The controller gives the wallet what it
/// lacks once it has stopped it, when the code can spend nothing more.
#[test]
fn a_wallet_its_code_drained_is_topped_up_by_the_controller_and_repaired() {
    let net = net();
    let members = net.members();
    let hostile = caller_wasm();
    let upgrade = InstallMode::Upgrade {
        skip_pre_upgrade: false,
    };
    let id = net.ready(
        &net.install_of(&hostile, upgrade, no_arg(), None),
        &[11, 12],
        Some(&hostile),
    );
    net.execute(12, id).unwrap();
    net.drain(60_000_000_000);
    let liquid = net.wallet_liquid_cycles();
    assert!(liquid < 150_000_000_000, "{liquid}");

    let repaired = wallet_wasm();
    let repair = net.install_of(&repaired, upgrade, upgrade_arg(), None);
    let id = net.ready(&repair, &[11, 12], Some(&repaired));
    let paid_before = net.state().cycles;
    let executed = net.execute(11, id).unwrap();
    assert!(matches!(executed.status, OperationStatus::Executed { .. }));
    assert_eq!(net.wallet_module(), sha256(&repaired));
    assert!(net.wallet_running());
    assert_eq!(net.members(), members);
    // It paid what the wallet lacked of the 350B an install needs.
    let paid = u128::try_from((paid_before - net.state().cycles).0).unwrap();
    assert!(paid >= 350_000_000_000 - liquid, "{paid}");
}

/// Wallet code can keep a call in flight forever, so the wallet never stops
/// and no upgrade can run. A reinstall uninstalls that code, which drops the
/// call, and the controller pays what the wallet lacks. The wallet starts
/// over at the same canister id, and its owners claim it again: an
/// account's address comes from the wallet's id and the account's index, so
/// the funds are where they were, and the wallet spends them.
#[test]
fn code_that_never_lets_the_wallet_stop_is_reinstalled_and_the_funds_stay() {
    let pic = PocketIcBuilder::new()
        .with_nns_subnet()
        .with_application_subnet()
        .build();
    let net = net_with(pic);
    let ledger =
        net.pic
            .create_canister_on_subnet(None, None, net.pic.topology().get_nns().unwrap());
    ledgers::install(&net.pic, ledger, net.wallet);
    let account = ledgers::add_token_account(&net, ledger);
    let accounts = ledgers::accounts(&net);
    assert_eq!(ledgers::balance(&net.pic, ledger, &account), 1_000);

    let hostile = caller_wasm();
    let upgrade = InstallMode::Upgrade {
        skip_pre_upgrade: false,
    };
    let id = net.ready(
        &net.install_of(&hostile, upgrade, no_arg(), None),
        &[11, 12],
        Some(&hostile),
    );
    net.execute(12, id).unwrap();
    net.drain(60_000_000_000);
    // The code calls a canister that never answers, and keeps that call in
    // flight.
    let spinner = caller_canister(&net.pic);
    let held = net
        .pic
        .submit_call(
            net.wallet,
            user(99),
            "forward",
            encode_args((spinner, "spin", ByteBuf::from(encode_one(spinner).unwrap()))).unwrap(),
        )
        .unwrap();
    for _ in 0..5 {
        net.pic.tick();
    }
    assert!(net.pic.ingress_status(held.clone()).is_none());

    // An upgrade stops the wallet first, which this code never lets happen:
    // the IC gives up on the stop, and the controller starts the wallet again.
    let repaired = wallet_wasm();
    let repair = net.install_of(&repaired, upgrade, upgrade_arg(), None);
    let id = net.ready(&repair, &[11, 12], Some(&repaired));
    let refused = net.execute_slowly(11, id);
    assert!(
        matches!(&refused, Err(ControllerError::ExecutionFailed(reason)) if reason.contains("did not stop")),
        "{refused:?}"
    );
    assert_eq!(net.wallet_module(), sha256(&hostile));
    assert!(net.wallet_running());
    assert_eq!(net.operation(id).status, OperationStatus::Ready);
    assert!(net.pic.ingress_status(held.clone()).is_none());

    // A reinstall, with a new claim key the owners hold.
    let claim_key = SigningKey::from_bytes(&[2; 32]);
    let init = WalletArg::Init(InitArgs {
        claim_public_key: ByteBuf::from(claim_key.verifying_key().to_bytes()),
        claim_expires_at_ns: net.now() + 30 * DAY_NS,
        release_channel: net.pic.create_canister(),
        release_signers: SIGNERS.iter().map(|&seed| public(seed)).collect(),
        release_threshold: 2,
        factory: None,
    });
    let reinstall = net.install_of(
        &repaired,
        InstallMode::Reinstall,
        ByteBuf::from(encode_one(init).unwrap()),
        None,
    );
    let id = net.ready(&reinstall, &[12, 13], Some(&repaired));
    assert!(net.wallet_liquid_cycles() < 150_000_000_000);
    let paid_before = net.state().cycles;
    let executed = net.execute(13, id).unwrap();
    assert!(matches!(executed.status, OperationStatus::Executed { .. }));
    assert_eq!(net.wallet_module(), sha256(&repaired));
    assert!(net.wallet_running());
    // Uninstalling freed the wallet's memory, and with it part of the
    // reserve its freezing threshold keeps, so it lacked less than before.
    let paid = u128::try_from((paid_before - net.state().cycles).0).unwrap();
    assert!((200_000_000_000..400_000_000_000).contains(&paid), "{paid}");
    // The uninstall rejected the call the code held.
    assert!(matches!(net.pic.ingress_status(held), Some(Err(_))));
    assert!(!net.status().unwrap().claimed);

    let signature = ByteBuf::from(
        claim_key
            .sign(wallet::claim_message(net.wallet, net.owner).as_bytes())
            .to_bytes(),
    );
    net.wallet_call::<Result<Member, WalletError>>(
        "claim",
        encode_one(ClaimArgs { signature }).unwrap(),
    )
    .unwrap();
    let again = ledgers::add_token_account(&net, ledger);
    assert_eq!(ledgers::accounts(&net), accounts);
    assert_eq!(ledgers::balance(&net.pic, ledger, &again), 1_000);
    let transfer = net
        .propose_action(ledgers::transfer(again.id, user(20), 100))
        .unwrap();
    let sent = net.execute_action(transfer.id).unwrap();
    assert!(
        matches!(sent.state, ActionState::Executed(Outcome::Sent(_))),
        "{sent:?}"
    );
    assert_eq!(ledgers::balance(&net.pic, ledger, &again), 1_000 - 100 - 10);
}

#[test]
fn a_controller_short_of_cycles_refuses_until_topped_up() {
    // The test keeps the anonymous principal as the controller's controller,
    // to lower what it can spend once the operation is ready, and hands it
    // the wallet the way the factory will, without the wallet's action.
    let pic = PocketIc::new();
    let (wallet, owner) = claimed_wallet(&pic, 20_000_000_000_000);
    let controller = installed_controller(&pic, wallet, policy_of(&KEYS, true), 400_000_000_000);
    pic.set_controllers(wallet, Some(wallet), vec![controller])
        .unwrap();
    let net = Net {
        pic,
        wallet,
        controller,
        owner,
    };
    let wasm = release_of(&wallet_wasm(), "0.2.0");
    let operation = net.install_of(
        &wasm,
        InstallMode::Upgrade {
            skip_pre_upgrade: false,
        },
        upgrade_arg(),
        None,
    );
    let id = net.ready(&operation, &[11, 12], Some(&wasm));
    let reset = pocket_ic::CanisterSettings {
        freezing_threshold: Some(Nat::from(30 * 86_400u64)),
        ..pocket_ic::CanisterSettings::default()
    };
    spendable_only(&net.pic, controller, None, 60_000_000_000);
    let refused = net.execute(11, id);
    assert!(
        matches!(
            &refused,
            Err(ControllerError::InsufficientCycles { canister, needed, .. })
                if *canister == net.controller && u64::try_from(&needed.0) == Ok(110_000_000_000)
        ),
        "{refused:?}"
    );
    net.pic
        .update_canister_settings(controller, None, reset)
        .unwrap();
    // It can pay for its own calls, but when the wallet can spend less than
    // an install needs, it must also be able to pay what the wallet lacks.
    spendable_only(&net.pic, net.wallet, Some(net.controller), 20_000_000_000);
    let refused = net.execute(11, id);
    assert!(
        matches!(
            &refused,
            Err(ControllerError::InsufficientCycles { canister, needed, .. })
                if *canister == net.controller
                    && u64::try_from(&needed.0).is_ok_and(|needed| needed > 110_000_000_000 + 330_000_000_000)
        ),
        "{refused:?}"
    );
    // Refused before it stopped anything.
    assert!(net.wallet_running());
    assert_eq!(net.operation(id).status, OperationStatus::Ready);
    // Anyone can top up either. (The raised threshold also raises the
    // wallet's reserve as the install grows its memory, so the wallet's
    // top-up is large.)
    net.pic.add_cycles(net.wallet, 100_000_000_000_000);
    net.execute(11, id).unwrap();
    assert_eq!(net.wallet_module(), sha256(&wasm));
}

#[test]
fn replace_controller_hands_the_wallet_to_a_verified_new_controller() {
    let net = net();
    let next_policy = policy_of(&KEYS, false);
    let next = controller_for(&net.pic, net.wallet, next_policy.clone(), 5_000_000_000_000);
    let module = ByteBuf::from(sha256(&controller_wasm()).to_vec());

    // A controller for another policy than the approved one is refused.
    let wrong = net.operation_of(OperationKind::ReplaceController {
        controller: next,
        module_hash: module.clone(),
        policy_sha256: ByteBuf::from(policy_of(&KEYS, true).sha256().to_vec()),
    });
    let id = net.ready(&wrong, &[11, 12], None);
    assert!(matches!(
        net.execute(11, id),
        Err(ControllerError::ExecutionFailed(reason)) if reason.contains("another policy")
    ));
    net.cancel(11, id, cancellation(11, &wrong)).unwrap();

    // So is one whose creator left it settings that could strand the
    // wallet.
    let costly = installed_controller(&net.pic, net.wallet, next_policy.clone(), 5_000_000_000_000);
    net.pic
        .update_canister_settings(
            costly,
            None,
            pocket_ic::CanisterSettings {
                reserved_cycles_limit: Some(Nat::from(0u8)),
                ..pocket_ic::CanisterSettings::default()
            },
        )
        .unwrap();
    net.pic.set_controllers(costly, None, vec![]).unwrap();
    let to_costly = net.operation_of(OperationKind::ReplaceController {
        controller: costly,
        module_hash: module.clone(),
        policy_sha256: ByteBuf::from(next_policy.sha256().to_vec()),
    });
    let id = net.ready(&to_costly, &[11, 12], None);
    assert!(matches!(
        net.execute(11, id),
        Err(ControllerError::ExecutionFailed(reason)) if reason.contains("reserved cycles limit")
    ));
    net.cancel(11, id, cancellation(11, &to_costly)).unwrap();

    let replace = net.operation_of(OperationKind::ReplaceController {
        controller: next,
        module_hash: module,
        policy_sha256: ByteBuf::from(next_policy.sha256().to_vec()),
    });
    let id = net.ready(&replace, &[12, 13], None);
    let stale = net.install_of(
        &wallet_wasm(),
        InstallMode::Upgrade {
            skip_pre_upgrade: false,
        },
        upgrade_arg(),
        None,
    );
    let stale_id = net.propose(11, &stale, &[11]).unwrap().id;
    // A wallet that passed to the new controller some other way is not a
    // hand-over this controller made.
    net.pic
        .set_controllers(net.wallet, Some(net.controller), vec![next])
        .unwrap();
    assert!(matches!(
        net.execute(13, id),
        Err(ControllerError::ExecutionFailed(reason)) if reason.contains("does not control the wallet")
    ));
    assert_eq!(net.state().replaced_by, None);
    net.pic
        .set_controllers(net.wallet, Some(next), vec![net.controller])
        .unwrap();
    net.execute(13, id).unwrap();
    assert_eq!(net.wallet_controllers(), vec![next]);
    assert_eq!(net.state().replaced_by, Some(next));
    assert_eq!(net.operation(stale_id).status, OperationStatus::Stale);
    assert!(matches!(
        net.execute(11, stale_id),
        Err(ControllerError::Replaced(controller)) if controller == next
    ));

    // The new controller acts; the old one has no power left.
    let successor = Net {
        pic: net.pic,
        wallet: net.wallet,
        controller: next,
        owner: net.owner,
    };
    let wasm = release_of(&wallet_wasm(), "0.2.0");
    let operation = successor.install_of(
        &wasm,
        InstallMode::Upgrade {
            skip_pre_upgrade: false,
        },
        upgrade_arg(),
        None,
    );
    let id = successor.ready(&operation, &[11, 12], Some(&wasm));
    successor.execute(12, id).unwrap();
    assert_eq!(successor.wallet_module(), sha256(&wasm));
}

#[test]
fn a_controller_cannot_be_upgraded_even_by_whoever_still_controls_it() {
    let pic = PocketIc::new();
    let (wallet, _) = claimed_wallet(&pic, 1_000_000_000_000);
    let held = pic.create_canister();
    pic.add_cycles(held, 1_000_000_000_000);
    let init = encode_one(ControllerArg::Init(ControllerInit {
        wallet,
        proofs: proofs(held, wallet, &KEYS),
        policy: policy_of(&KEYS, true),
    }))
    .unwrap();
    pic.install_canister(held, controller_wasm(), init, None);
    // The IC limits how many instructions a canister's installs take in a
    // while; wait until it takes another.
    let upgraded = loop {
        let upgraded =
            pic.upgrade_canister(held, controller_wasm(), encode_args(()).unwrap(), None);
        match &upgraded {
            Err(reject) if reject.reject_message.contains("rate limited") => {
                pic.advance_time(Duration::from_secs(60));
                for _ in 0..10 {
                    pic.tick();
                }
            }
            _ => break upgraded,
        }
    };
    assert!(
        matches!(&upgraded, Err(reject) if reject.reject_message.contains("never upgraded")),
        "{upgraded:?}"
    );

    // A proof of possession for another controller or wallet is refused.
    let other = pic.create_canister();
    pic.add_cycles(other, 1_000_000_000_000);
    let init = encode_one(ControllerArg::Init(ControllerInit {
        wallet,
        proofs: proofs(held, wallet, &KEYS),
        policy: policy_of(&KEYS, true),
    }))
    .unwrap();
    let installed = pic.reinstall_canister(other, controller_wasm(), init, None);
    assert!(
        matches!(&installed, Err(reject) if reject.reject_message.contains("proof of possession")),
        "{installed:?}"
    );
}

/// Transaction safety across an install: a transfer and an install started
/// in the same round end with the transfer sent once, whichever order the
/// IC runs their calls in, and the wallet's accounts the same after the
/// install. Stopping the wallet waits for every call it has in flight.
#[test]
fn a_transfer_and_an_install_started_together_leave_the_transfer_sent_once() {
    let pic = PocketIcBuilder::new()
        .with_nns_subnet()
        .with_application_subnet()
        .build();
    let net = net_with(pic);
    let ledger =
        net.pic
            .create_canister_on_subnet(None, None, net.pic.topology().get_nns().unwrap());
    ledgers::install(&net.pic, ledger, net.wallet);
    let account = ledgers::add_token_account(&net, ledger);
    let before = ledgers::accounts(&net);
    assert_eq!(ledgers::balance(&net.pic, ledger, &account), 1_000);

    let wasm = release_of(&wallet_wasm(), "0.2.0");
    let operation = net.install_of(
        &wasm,
        InstallMode::Upgrade {
            skip_pre_upgrade: false,
        },
        upgrade_arg(),
        None,
    );
    let id = net.ready(&operation, &[11, 12], Some(&wasm));
    let transfer = net
        .propose_action(ledgers::transfer(account.id, user(20), 100))
        .unwrap();

    // Both start in the same round: the wallet calls the ledger on another
    // subnet while the controller reads the wallet's status and stops it.
    let sent = net
        .pic
        .submit_call(
            net.wallet,
            net.owner,
            "execute",
            encode_one(transfer.id).unwrap(),
        )
        .unwrap();
    let installing = net
        .pic
        .submit_call(net.controller, by(11), "execute", encode_one(id).unwrap())
        .unwrap();
    let sent: Result<Pending, WalletError> = decode("execute", net.pic.await_call(sent).unwrap());
    let installed: Result<OperationView, ControllerError> =
        decode("execute", net.pic.await_call(installing).unwrap());

    let sent = sent.unwrap();
    assert!(
        matches!(sent.state, ActionState::Executed(Outcome::Sent(_))),
        "{sent:?}"
    );
    assert!(matches!(
        installed.unwrap().status,
        OperationStatus::Executed { .. }
    ));
    assert_eq!(net.wallet_module(), sha256(&wasm));
    assert!(net.wallet_running());
    // Sent once: the amount and one fee left the account.
    assert_eq!(
        ledgers::balance(&net.pic, ledger, &account),
        1_000 - 100 - 10
    );
    // The same accounts, at the same addresses, and the wallet reads the
    // same balance the ledger does.
    assert_eq!(ledgers::accounts(&net), before);
    assert_eq!(ledgers::wallet_balance(&net, account.id), 1_000 - 100 - 10);
    let executed = net.execute_action(transfer.id);
    assert!(
        matches!(executed, Err(WalletError::InvalidArgument(_))),
        "{executed:?}"
    );
}

/// An ICRC-1 ledger for the transfer test, as CI downloads it.
mod ledgers {
    use super::*;
    use icrc_ledger_types::icrc1::account::Account as LedgerAccount;
    use wallet::{Account, AddAccountArgs, Balance, Chain, TransferRequest};

    #[derive(CandidType)]
    enum LedgerArg {
        Init(LedgerInit),
    }

    #[derive(CandidType)]
    struct FeatureFlags {
        icrc2: bool,
    }

    #[derive(CandidType)]
    struct ArchiveOptions {
        num_blocks_to_archive: u64,
        max_transactions_per_response: Option<u64>,
        trigger_threshold: u64,
        max_message_size_bytes: Option<u64>,
        cycles_for_archive_creation: Option<u64>,
        node_max_memory_size_bytes: Option<u64>,
        controller_id: Principal,
        more_controller_ids: Option<Vec<Principal>>,
    }

    #[derive(CandidType)]
    #[allow(dead_code)]
    enum MetadataValue {
        Nat(Nat),
    }

    #[derive(CandidType)]
    struct LedgerInit {
        minting_account: LedgerAccount,
        fee_collector_account: Option<LedgerAccount>,
        transfer_fee: Nat,
        decimals: Option<u8>,
        max_memo_length: Option<u16>,
        token_symbol: String,
        token_name: String,
        metadata: Vec<(String, MetadataValue)>,
        initial_balances: Vec<(LedgerAccount, Nat)>,
        feature_flags: Option<FeatureFlags>,
        archive_options: ArchiveOptions,
        index_principal: Option<Principal>,
    }

    fn wasm() -> Vec<u8> {
        let path = std::env::var("ICRC1_LEDGER_WASM").expect(
            "ICRC1_LEDGER_WASM names ic-icrc1-ledger.wasm.gz from the dfinity/ic release \
             ledger-suite-icrc-2025-09-01",
        );
        std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
    }

    /// A ledger whose fee is 10 and that gives the wallet's account 0, its
    /// principal with no subaccount, 1,000.
    pub fn install(pic: &PocketIc, ledger: Principal, wallet: Principal) {
        pic.add_cycles(ledger, 2_000_000_000_000);
        let init = LedgerInit {
            minting_account: LedgerAccount::from(user(100)),
            fee_collector_account: None,
            transfer_fee: Nat::from(10u64),
            decimals: Some(8),
            max_memo_length: Some(32),
            token_symbol: "TKN".to_string(),
            token_name: "TKN".to_string(),
            metadata: vec![],
            initial_balances: vec![(LedgerAccount::from(wallet), Nat::from(1_000u64))],
            feature_flags: Some(FeatureFlags { icrc2: false }),
            archive_options: ArchiveOptions {
                num_blocks_to_archive: 1_000,
                max_transactions_per_response: None,
                trigger_threshold: 2_000,
                max_message_size_bytes: None,
                cycles_for_archive_creation: None,
                node_max_memory_size_bytes: None,
                controller_id: user(100),
                more_controller_ids: None,
            },
            index_principal: None,
        };
        pic.install_canister(
            ledger,
            wasm(),
            encode_one(LedgerArg::Init(init)).unwrap(),
            None,
        );
    }

    pub fn add_token_account(net: &Net, ledger: Principal) -> Account {
        net.wallet_call::<Result<Account, WalletError>>(
            "add_account",
            encode_one(AddAccountArgs {
                chain: Chain::Icrc(ledger),
                label: "tokens".to_string(),
            })
            .unwrap(),
        )
        .unwrap()
    }

    pub fn accounts(net: &Net) -> Vec<Account> {
        let reply = net
            .pic
            .query_call(net.wallet, net.owner, "accounts", encode_one(()).unwrap())
            .unwrap();
        decode::<Result<Vec<Account>, WalletError>>("accounts", reply).unwrap()
    }

    pub fn balance(pic: &PocketIc, ledger: Principal, account: &Account) -> u64 {
        let owner: LedgerAccount = account.address.parse().unwrap();
        let reply = pic
            .query_call(
                ledger,
                Principal::anonymous(),
                "icrc1_balance_of",
                encode_one(owner).unwrap(),
            )
            .unwrap();
        u64::try_from(decode::<Nat>("icrc1_balance_of", reply).0).unwrap()
    }

    pub fn wallet_balance(net: &Net, account: u32) -> u64 {
        let balance = net
            .wallet_call::<Result<Balance, WalletError>>("balance", encode_one(account).unwrap())
            .unwrap();
        u64::try_from(balance.amount.0).unwrap()
    }

    pub fn transfer(from: u32, to: Principal, amount: u64) -> Action {
        Action::Transfer(TransferRequest {
            from,
            to: LedgerAccount::from(to).to_string(),
            amount: Nat::from(amount),
            fee: None,
            memo: None,
        })
    }
}
