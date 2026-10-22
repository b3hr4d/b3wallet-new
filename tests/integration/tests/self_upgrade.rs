//! Step 11: the real wallet upgrades itself to signed releases that the real
//! release channel serves. Build both first:
//! `cargo build -p wallet -p release_channel --target wasm32-unknown-unknown --release`.
//! PocketIC uses the server binary named by `POCKET_IC_BIN`.
//!
//! A release here is the wallet's own Wasm with a custom section that names
//! it: another module, with the same code.

use candid::{CandidType, Deserialize, Principal, decode_one, encode_one};
use ed25519_dalek::{Signer, SigningKey};
use pocket_ic::{CreateCanisterParams, PocketIc};
use release::channel::{ChannelArg, ChannelConfig, ChannelError};
use release::{ReleaseManifest, sha256};
use release_tool::chunk_args;
use serde_bytes::ByteBuf;
use std::time::Duration;
use wallet::{
    Action, ActionId, ActionState, AuditEntry, AuditEvent, AuditRequest, ClaimArgs,
    FIRST_RESEND_NS, InitArgs, MAX_INSTALL_SENDS, Outcome, Pending, Role, Status, UpgradeState,
    WalletArg, WalletError, cycles_to_upgrade,
};

const HOUR_NS: u64 = 60 * 60 * 1_000_000_000;
const DAY_NS: u64 = 24 * HOUR_NS;

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

fn channel_wasm() -> Vec<u8> {
    read_wasm("RELEASE_CHANNEL_WASM", "release_channel")
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

/// `wasm` with a custom section that names `release`.
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
    // One type, () -> (); one function of it.
    module.extend([0x01, 0x04, 0x01, 0x60, 0x00, 0x00]);
    module.extend([0x03, 0x02, 0x01, 0x00]);
    // Exported as canister_post_upgrade.
    module.extend([0x07, 0x19, 0x01, 0x15]);
    module.extend(b"canister_post_upgrade");
    module.extend([0x00, 0x00]);
    // Its body: unreachable.
    module.extend([0x0a, 0x05, 0x01, 0x03, 0x00, 0x00, 0x0b]);
    module
}

fn signer(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn public_key(seed: u8) -> ByteBuf {
    ByteBuf::from(signer(seed).verifying_key().to_bytes())
}

fn user(seed: u8) -> Principal {
    Principal::self_authenticating([seed; 32])
}

fn publisher() -> Principal {
    user(40)
}

fn claim_key() -> SigningKey {
    SigningKey::from_bytes(&[1; 32])
}

struct Net {
    pic: PocketIc,
    channel: Principal,
    wallet: Principal,
    owner: Principal,
}

/// A channel that publishes what two of `channel_signers` signed, and a
/// claimed wallet that pins the keys 1, 2 and 3, two of which must sign.
/// The wallet is its own only controller unless `self_owned` is false.
fn net(channel_signers: [u8; 3], self_owned: bool) -> Net {
    net_with_cycles(channel_signers, self_owned, 200_000_000_000_000)
}

/// `net`, with a wallet given `cycles`.
fn net_with_cycles(channel_signers: [u8; 3], self_owned: bool, cycles: u128) -> Net {
    let pic = PocketIc::new();
    let channel = pic.create_canister();
    pic.add_cycles(channel, 10_000_000_000_000);
    let config = ChannelConfig {
        publishers: vec![publisher()],
        release_signers: channel_signers
            .iter()
            .map(|&seed| public_key(seed))
            .collect(),
        release_threshold: 2,
    };
    pic.install_canister(
        channel,
        channel_wasm(),
        encode_one(ChannelArg::Init(config)).unwrap(),
        None,
    );

    let wallet = pic
        .create_canister_with_params(
            None,
            CreateCanisterParams {
                cycles: Some(cycles),
                ..CreateCanisterParams::default()
            },
        )
        .unwrap();
    let args = InitArgs {
        claim_public_key: ByteBuf::from(claim_key().verifying_key().to_bytes()),
        claim_expires_at_ns: pic.get_time().as_nanos_since_unix_epoch() + DAY_NS,
        release_channel: channel,
        release_signers: vec![public_key(1), public_key(2), public_key(3)],
        release_threshold: 2,
        factory: None,
        bitcoin_network: None,
    };
    pic.install_canister(
        wallet,
        wallet_wasm(),
        encode_one(WalletArg::Init(args)).unwrap(),
        None,
    );
    // PocketIC installs a module this large from the wallet's own chunk
    // store. Empty it, so the tests see only the chunks the wallet stages.
    pic.clear_chunk_store(wallet, None).unwrap();
    if self_owned {
        pic.set_controllers(wallet, None, vec![wallet]).unwrap();
    }
    let owner = user(7);
    let message = wallet::claim_message(wallet, owner);
    let signature = ByteBuf::from(claim_key().sign(message.as_bytes()).to_bytes());
    let claimed: Result<wallet::Member, WalletError> = update(
        &pic,
        wallet,
        owner,
        "claim",
        encode_one(ClaimArgs { signature }).unwrap(),
    );
    claimed.unwrap();
    Net {
        pic,
        channel,
        wallet,
        owner,
    }
}

fn update<R: CandidType + for<'de> Deserialize<'de>>(
    pic: &PocketIc,
    id: Principal,
    caller: Principal,
    method: &str,
    arg: Vec<u8>,
) -> R {
    let reply = pic
        .update_call(id, caller, method, arg)
        .unwrap_or_else(|reject| panic!("{method} answers: {reject}"));
    decode_one(&reply).unwrap()
}

fn query<R: CandidType + for<'de> Deserialize<'de>>(
    pic: &PocketIc,
    id: Principal,
    caller: Principal,
    method: &str,
    arg: Vec<u8>,
) -> R {
    let reply = pic
        .query_call(id, caller, method, arg)
        .unwrap_or_else(|reject| panic!("{method} answers: {reject}"));
    decode_one(&reply).unwrap()
}

impl Net {
    /// Publishes `wasm` as `version`, tested to upgrade from `tested_from`
    /// and signed by `seeds`, the way `release-tool` and icp-cli do.
    fn publish(
        &self,
        version: &str,
        wasm: &[u8],
        tested_from: Vec<[u8; 32]>,
        seeds: &[u8],
    ) -> ReleaseManifest {
        for arg in chunk_args(version, wasm) {
            let published: Result<(), ChannelError> =
                update(&self.pic, self.channel, publisher(), "publish_chunk", arg);
            assert_eq!(published, Ok(()));
        }
        let mut manifest = ReleaseManifest::new(
            version.to_string(),
            wasm,
            tested_from,
            format!("https://example.org/releases/{version}"),
            self.pic.get_time().as_nanos_since_unix_epoch(),
        );
        for &seed in seeds {
            manifest.sign(&signer(seed));
        }
        let published: Result<(), ChannelError> = update(
            &self.pic,
            self.channel,
            publisher(),
            "publish_manifest",
            encode_one(&manifest).unwrap(),
        );
        assert_eq!(published, Ok(()));
        manifest
    }

    fn module_hash(&self) -> Vec<u8> {
        self.pic
            .canister_status(self.wallet, Some(self.wallet))
            .unwrap()
            .module_hash
            .unwrap()
    }

    fn stored_chunks(&self) -> usize {
        self.pic
            .stored_chunks(self.wallet, Some(self.wallet))
            .unwrap()
            .len()
    }

    /// Whether the new version's timer clears the chunk store within a few
    /// rounds of the install: how many the install takes to commit depends
    /// on the module's size and its `post_upgrade`.
    fn clears_the_chunk_store(&self) -> bool {
        (0..10).any(|_| {
            let cleared = self.stored_chunks() == 0;
            self.pic.tick();
            cleared
        })
    }

    fn release_available(&self) -> Result<Option<ReleaseManifest>, WalletError> {
        update(
            &self.pic,
            self.wallet,
            self.owner,
            "release_available",
            encode_one(()).unwrap(),
        )
    }

    fn upgrade_status(&self) -> UpgradeState {
        query(
            &self.pic,
            self.wallet,
            self.owner,
            "upgrade_status",
            encode_one(()).unwrap(),
        )
    }

    fn status(&self) -> Status {
        query(
            &self.pic,
            self.wallet,
            Principal::anonymous(),
            "status",
            encode_one(()).unwrap(),
        )
    }

    fn propose(&self, action: Action) -> Result<Pending, WalletError> {
        update(
            &self.pic,
            self.wallet,
            self.owner,
            "propose",
            encode_one(action).unwrap(),
        )
    }

    /// `execute` or `reject`.
    fn act(&self, method: &str, id: ActionId) -> Result<Pending, WalletError> {
        update(
            &self.pic,
            self.wallet,
            self.owner,
            method,
            encode_one(id).unwrap(),
        )
    }

    /// Proposes the upgrade to `manifest` and waits out its timelock.
    fn ready_upgrade(&self, manifest: &ReleaseManifest) -> ActionId {
        let proposed = self
            .propose(Action::Upgrade {
                version: manifest.version.clone(),
                wasm_sha256: manifest.wasm_sha256.clone(),
            })
            .unwrap();
        assert_eq!(proposed.state, ActionState::Ready);
        assert!(matches!(
            self.act("execute", proposed.id),
            Err(WalletError::TimelockActive { .. })
        ));
        self.pic.advance_time(Duration::from_nanos(7 * DAY_NS + 1));
        proposed.id
    }

    /// The traps the wallet's log holds, one for each install of a release
    /// whose `post_upgrade` traps.
    fn traps(&self) -> usize {
        self.pic
            .fetch_canister_logs(self.wallet, self.wallet)
            .unwrap()
            .iter()
            .filter(|record| record.content.starts_with(b"[TRAP]"))
            .count()
    }

    /// The rounds in which the IC runs the install and the timer after it.
    fn rounds(&self) {
        for _ in 0..4 {
            self.pic.tick();
        }
    }

    /// The audit entries after the first `skip`, as who and what.
    fn audit_since(&self, skip: usize) -> Vec<(Option<Principal>, AuditEvent)> {
        let page: Result<Vec<AuditEntry>, WalletError> = query(
            &self.pic,
            self.wallet,
            self.owner,
            "audit_log",
            encode_one(AuditRequest {
                before_seq: None,
                limit: 100,
            })
            .unwrap(),
        );
        let mut entries = page.unwrap();
        entries.reverse();
        entries
            .into_iter()
            .skip(skip)
            .map(|entry| (entry.by, entry.event))
            .collect()
    }

    fn audit_len(&self) -> usize {
        self.audit_since(0).len()
    }

    /// Adds a member at once: the wallet still runs its actions.
    fn still_runs(&self, seed: u8) {
        let proposed = self
            .propose(Action::AddMember {
                id: user(seed),
                role: Role::Viewer,
                label: "phone".to_string(),
            })
            .unwrap();
        let executed = self.act("execute", proposed.id).unwrap();
        assert_eq!(executed.state, ActionState::Executed(Outcome::Done));
    }
}

/// An upgrade action's id, from an `execute` refused because it ended.
fn ended(net: &Net, id: ActionId) -> String {
    match net.act("execute", id) {
        Err(WalletError::InvalidArgument(reason)) => reason,
        other => panic!("action {id} has not ended: {other:?}"),
    }
}

#[test]
fn the_wallet_upgrades_itself_to_the_next_release_and_the_one_after() {
    let net = net([1, 2, 3], true);
    let first = wallet_wasm();
    assert_eq!(net.module_hash(), sha256(&first));
    assert_eq!(net.release_available(), Ok(None));

    let mut running = first.clone();
    for (version, seeds) in [("0.2.0", [1, 2]), ("0.3.0", [2, 3])] {
        let wasm = release_of(&first, version);
        let release = net.publish(version, &wasm, vec![sha256(&running)], &seeds);
        assert!(
            release.chunk_hashes.len() > 1,
            "a release of several chunks"
        );
        assert_eq!(net.release_available(), Ok(Some(release.clone())));
        let id = net.ready_upgrade(&release);
        let logged = net.audit_len();

        let sent = net.act("execute", id).unwrap();
        assert_eq!(sent.state, ActionState::Ready);
        let installing = net.status().upgrade;
        assert!(
            matches!(
                &installing,
                UpgradeState::Installing { version: v, wasm_sha256, deadline_ns, started_at_ns }
                    if v == version
                        && wasm_sha256 == &release.wasm_sha256
                        && *deadline_ns == started_at_ns + HOUR_NS
            ),
            "{installing:?}"
        );

        net.rounds();
        assert_eq!(net.module_hash(), sha256(&wasm));
        assert_eq!(net.upgrade_status(), UpgradeState::Idle);
        assert_eq!(net.status().upgrade, UpgradeState::Idle);
        assert_eq!(ended(&net, id), format!("action {id} is already executed"));
        assert_eq!(
            net.audit_since(logged),
            [
                (
                    Some(net.owner),
                    AuditEvent::UpgradeStaged(version.to_string())
                ),
                (None, AuditEvent::Executed(id)),
                (None, AuditEvent::UpgradeInstalled(version.to_string())),
            ]
        );
        assert!(
            net.clears_the_chunk_store(),
            "the timer clears the chunk store"
        );
        assert_eq!(net.release_available(), Ok(None));
        running = wasm;
    }
    // The state came along both times.
    assert!(net.status().claimed);
    net.still_runs(8);
}

#[test]
fn a_release_whose_post_upgrade_traps_leaves_the_old_version_running_and_failed() {
    let net = net([1, 2, 3], true);
    let first = wallet_wasm();
    let broken = net.publish("0.9.0", &trapping_module(), vec![sha256(&first)], &[1, 2]);
    let id = net.ready_upgrade(&broken);
    let logged = net.audit_len();
    net.act("execute", id).unwrap();

    net.rounds();
    // The install trapped and the old version runs, waiting for its deadline.
    assert_eq!(net.module_hash(), sha256(&first));
    assert!(matches!(
        net.upgrade_status(),
        UpgradeState::Installing { .. }
    ));
    let other = net
        .propose(Action::AddMember {
            id: user(9),
            role: Role::Viewer,
            label: "phone".to_string(),
        })
        .unwrap();
    assert_eq!(
        net.act("execute", other.id),
        Err(WalletError::UpgradeInProgress)
    );
    assert_eq!(net.act("reject", id), Err(WalletError::UpgradeInProgress));
    assert_eq!(net.release_available(), Err(WalletError::UpgradeInProgress));

    net.pic.advance_time(Duration::from_nanos(HOUR_NS));
    net.rounds();
    let failed = net.upgrade_status();
    assert!(
        matches!(
            &failed,
            UpgradeState::Failed { version, reason, .. }
                if version == "0.9.0" && reason.contains("post_upgrade trapped")
        ),
        "{failed:?}"
    );
    assert_eq!(net.status().upgrade, failed);
    assert_eq!(ended(&net, id), format!("action {id} is already failed"));
    assert_eq!(
        net.audit_since(logged),
        [
            (
                Some(net.owner),
                AuditEvent::UpgradeStaged("0.9.0".to_string())
            ),
            (Some(net.owner), AuditEvent::Proposed(other.id)),
            (None, AuditEvent::Executed(id)),
            (None, AuditEvent::UpgradeFailed("0.9.0".to_string())),
        ]
    );
    assert_eq!(net.stored_chunks(), 0);
    assert_eq!(net.module_hash(), sha256(&first));

    // The old version goes on, and installs the next release.
    let executed = net.act("execute", other.id).unwrap();
    assert_eq!(executed.state, ActionState::Executed(Outcome::Done));
    let fixed = release_of(&first, "1.0.0");
    let release = net.publish("1.0.0", &fixed, vec![sha256(&first)], &[1, 3]);
    let id = net.ready_upgrade(&release);
    net.act("execute", id).unwrap();
    net.rounds();
    assert_eq!(net.module_hash(), sha256(&fixed));
    assert_eq!(net.upgrade_status(), UpgradeState::Idle);
}

#[test]
fn a_release_the_wallet_would_not_install_ends_its_action_failed() {
    // The channel publishes what two of the keys 1, 4 and 5 signed; the
    // wallet installs what two of the keys 1, 2 and 3 signed.
    let net = net([1, 4, 5], true);
    let first = wallet_wasm();
    let tested = vec![sha256(&first)];

    let one_signer = net.publish(
        "0.2.0",
        &release_of(&first, "0.2.0"),
        tested.clone(),
        &[1, 4],
    );
    let refused = net.release_available();
    assert!(
        matches!(&refused, Err(WalletError::ReleaseRejected(reason)) if reason.contains("1 of the 2 signatures")),
        "{refused:?}"
    );
    let untested = net.publish(
        "0.3.0",
        &release_of(&first, "0.3.0"),
        vec![[9; 32]],
        &[1, 2, 4],
    );
    let refused = net.release_available();
    assert!(
        matches!(&refused, Err(WalletError::ReleaseRejected(reason)) if reason.contains("not tested")),
        "{refused:?}"
    );
    let good = net.publish("0.4.0", &release_of(&first, "0.4.0"), tested, &[2, 3, 4, 5]);
    assert_eq!(net.release_available(), Ok(Some(good.clone())));

    let other_wasm = Action::Upgrade {
        version: good.version.clone(),
        wasm_sha256: ByteBuf::from(vec![7; 32]),
    };
    let cases = [
        (one_signer.clone(), None, "1 of the 2 signatures"),
        (untested, None, "not tested to upgrade"),
        (good, Some(other_wasm), "not the Wasm the action names"),
    ];
    for (release, action, reason) in cases {
        let action = action.unwrap_or(Action::Upgrade {
            version: release.version.clone(),
            wasm_sha256: release.wasm_sha256.clone(),
        });
        let proposed = net.propose(action).unwrap();
        net.pic.advance_time(Duration::from_nanos(7 * DAY_NS + 1));
        let executed = net.act("execute", proposed.id).unwrap();
        assert!(
            matches!(&executed.state, ActionState::Failed(why) if why.contains(reason)),
            "{:?}",
            executed.state
        );
        assert_eq!(net.upgrade_status(), UpgradeState::Idle);
    }

    // A version the channel never published: the action waits for it.
    let missing = net
        .propose(Action::Upgrade {
            version: "7.7.7".to_string(),
            wasm_sha256: one_signer.wasm_sha256.clone(),
        })
        .unwrap();
    net.pic.advance_time(Duration::from_nanos(7 * DAY_NS + 1));
    assert!(matches!(
        net.act("execute", missing.id),
        Err(WalletError::ReleaseRejected(reason)) if reason.contains("no release 7.7.7")
    ));
    net.act("reject", missing.id).unwrap();

    // Proposals name a release in form.
    for (version, hash) in [("", 32), ("1 0", 32), ("1.0.0", 31)] {
        assert!(matches!(
            net.propose(Action::Upgrade {
                version: version.to_string(),
                wasm_sha256: ByteBuf::from(vec![0; hash]),
            }),
            Err(WalletError::InvalidArgument(_))
        ));
    }
    assert_eq!(net.module_hash(), sha256(&first));
    assert_eq!(net.stored_chunks(), 0);
    net.still_runs(8);
}

#[test]
fn a_wallet_that_does_not_control_itself_stages_nothing_until_it_does() {
    let net = net([1, 2, 3], false);
    let first = wallet_wasm();
    let wasm = release_of(&first, "0.2.0");
    let release = net.publish("0.2.0", &wasm, vec![sha256(&first)], &[1, 2]);
    let id = net.ready_upgrade(&release);
    let refused = net.act("execute", id);
    assert!(
        matches!(&refused, Err(WalletError::Forbidden(reason)) if reason.contains("not its own controller")),
        "{refused:?}"
    );
    // The action waits, and nothing was staged.
    assert_eq!(net.upgrade_status(), UpgradeState::Idle);
    let stored = net.pic.stored_chunks(net.wallet, None).unwrap();
    assert!(stored.is_empty());
    net.still_runs(8);

    // Once it controls itself, the same action installs the release.
    net.pic
        .set_controllers(net.wallet, None, vec![net.wallet])
        .unwrap();
    net.act("execute", id).unwrap();
    net.rounds();
    assert_eq!(net.module_hash(), sha256(&wasm));
    assert_eq!(net.upgrade_status(), UpgradeState::Idle);
}

#[test]
fn the_wallet_installs_only_once_its_calls_have_returned() {
    let net = net([1, 2, 3], true);
    let first = wallet_wasm();
    let wasm = release_of(&first, "0.2.0");
    let release = net.publish("0.2.0", &wasm, vec![sha256(&first)], &[1, 2]);
    let total = u32::try_from(release.chunk_hashes.len()).unwrap();
    let id = net.ready_upgrade(&release);
    let execute = net
        .pic
        .submit_call(net.wallet, net.owner, "execute", encode_one(id).unwrap())
        .unwrap();
    // While the last chunk is on its way, a member asks the channel for the
    // latest release, so that call is in flight when the install would go.
    let mut asked = None;
    let executed = loop {
        net.pic.tick();
        if asked.is_none()
            && matches!(
                net.upgrade_status(),
                UpgradeState::Staging { chunks_uploaded, .. } if chunks_uploaded + 1 == total
            )
        {
            let ask = net
                .pic
                .submit_call(
                    net.wallet,
                    net.owner,
                    "release_available",
                    encode_one(()).unwrap(),
                )
                .unwrap();
            asked = Some(ask);
        }
        if let Some(reply) = net.pic.ingress_status(execute.clone()) {
            let executed: Result<Pending, WalletError> = decode_one(&reply.unwrap()).unwrap();
            break executed;
        }
    };
    assert!(
        matches!(&executed, Err(WalletError::InvalidArgument(reason)) if reason.contains("waits for the answers to 1 of its calls")),
        "{executed:?}"
    );
    assert_eq!(
        net.upgrade_status(),
        UpgradeState::Staging {
            version: "0.2.0".to_string(),
            chunks_uploaded: total,
            chunks_total: total,
        }
    );
    assert_eq!(net.module_hash(), sha256(&first));
    let available: Result<Option<ReleaseManifest>, WalletError> =
        decode_one(&net.pic.await_call(asked.unwrap()).unwrap()).unwrap();
    assert_eq!(available, Ok(Some(release)));

    // The staged chunks stay; executing again installs them.
    let logged = net.audit_len();
    net.act("execute", id).unwrap();
    net.rounds();
    assert_eq!(net.module_hash(), sha256(&wasm));
    assert_eq!(
        net.audit_since(logged),
        [
            (None, AuditEvent::Executed(id)),
            (None, AuditEvent::UpgradeInstalled("0.2.0".to_string())),
        ]
    );
}

#[test]
fn an_install_the_old_version_outlives_is_sent_again_until_its_deadline() {
    let net = net([1, 2, 3], true);
    let first = wallet_wasm();
    let broken = net.publish("0.9.0", &trapping_module(), vec![sha256(&first)], &[1, 2]);
    let id = net.ready_upgrade(&broken);
    net.act("execute", id).unwrap();
    net.rounds();
    assert_eq!(net.traps(), 1);
    let UpgradeState::Installing { started_at_ns, .. } = net.upgrade_status() else {
        panic!("the install is on its way");
    };

    // Each time the IC refuses it, the old version sends it again: 1, 3, 7,
    // 15 and 31 minutes after the first send.
    let advance_to = |at: u64| {
        let now = net.pic.get_time().as_nanos_since_unix_epoch();
        net.pic.advance_time(Duration::from_nanos(at - now));
        net.rounds();
    };
    let mut sends = 1;
    for minutes in [1, 3, 7, 15, 31] {
        // Each round moves PocketIC's clock on a nanosecond: stop a second
        // short.
        advance_to(started_at_ns + minutes * FIRST_RESEND_NS - 1_000_000_000);
        assert_eq!(net.traps(), sends, "not yet at {minutes} minutes");
        advance_to(started_at_ns + minutes * FIRST_RESEND_NS);
        sends += 1;
        assert_eq!(net.traps(), sends, "sent again at {minutes} minutes");
        assert!(matches!(
            net.upgrade_status(),
            UpgradeState::Installing { .. }
        ));
    }
    assert_eq!(sends, MAX_INSTALL_SENDS as usize);

    advance_to(started_at_ns + HOUR_NS);
    assert_eq!(net.traps(), sends, "no send after the last");
    assert!(matches!(net.upgrade_status(), UpgradeState::Failed { .. }));
    assert_eq!(ended(&net, id), format!("action {id} is already failed"));
    assert_eq!(net.module_hash(), sha256(&first));
}

#[test]
fn an_upgrade_the_wallet_cannot_pay_for_stops_until_it_has_the_cycles() {
    // Enough to install the wallet, which an empty canister keeps nothing
    // back for, and not enough to stage and install a release with what the
    // wallet then keeps to stay running.
    let net = net_with_cycles([1, 2, 3], true, 360_000_000_000);
    let first = wallet_wasm();
    let wasm = release_of(&first, "0.2.0");
    let release = net.publish("0.2.0", &wasm, vec![sha256(&first)], &[1, 2]);
    let id = net.ready_upgrade(&release);

    let chunks = release.chunk_hashes.len() as u32;
    let refused = net.act("execute", id);
    let Err(WalletError::InsufficientCycles { available, needed }) = refused else {
        panic!("{refused:?}");
    };
    let available = u128::try_from(&available.0).unwrap();
    let needed = u128::try_from(&needed.0).unwrap();
    assert_eq!(needed, cycles_to_upgrade(chunks));
    assert!(available < needed);
    // Nothing staged, and the action still ready.
    assert_eq!(net.upgrade_status(), UpgradeState::Idle);
    assert_eq!(net.stored_chunks(), 0);

    // Topped up by what it said it lacks, and a little for the calls before
    // the check, the wallet stages the release, still has what the install
    // needs, and the IC installs it: the needs the wallet checks are enough.
    net.pic
        .add_cycles(net.wallet, needed - available + 1_000_000_000);
    let sent = net.act("execute", id).unwrap();
    assert_eq!(sent.state, ActionState::Ready);
    net.rounds();
    assert_eq!(net.module_hash(), sha256(&wasm));
    assert_eq!(net.upgrade_status(), UpgradeState::Idle);
}
