//! Step 12: the real factory creates wallets from the real release channel,
//! paid through the cycles ledger, and keeps no rights over any of them.
//! Build the canisters first:
//! `cargo build -p wallet -p release_channel -p factory --target wasm32-unknown-unknown --release`.
//! PocketIC uses the server binary named by `POCKET_IC_BIN`, and deploys the
//! cycles ledger and the cycles minting canister itself.

use candid::{CandidType, Deserialize, Nat, Principal, decode_one, encode_one};
use ed25519_dalek::{Signer, SigningKey};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use pocket_ic::common::rest::{IcpFeatures, IcpFeaturesConfig};
use pocket_ic::{CanisterSettings, PocketIc, PocketIcBuilder};
use release::channel::{ChannelArg, ChannelConfig, ChannelError};
use release::factory::{
    CreateWalletArgs, CreatedWallet, FactoryArg, FactoryConfig, FactoryError, MIN_CYCLES,
};
use release::{ReleaseManifest, sha256};
use release_tool::chunk_args;
use serde_bytes::ByteBuf;
use std::time::Duration;
use wallet::{Action, ActionState, ClaimArgs, Member, Role, Status, UpgradeState, WalletError};

const HOUR_NS: u64 = 60 * 60 * 1_000_000_000;
const DAY_NS: u64 = 24 * HOUR_NS;
const T: u128 = 1_000_000_000_000;
/// The cycles ledger's fee for each transfer, approval and creation.
const FEE: u128 = 100_000_000;

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

fn factory_wasm() -> Vec<u8> {
    read_wasm("FACTORY_WASM", "factory")
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

/// `wasm` with a custom section that names `release`: another module, with
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

/// A module whose `canister_init` traps, and nothing else.
fn trapping_init_module() -> Vec<u8> {
    let mut module = b"\0asm\x01\0\0\0".to_vec();
    // One type, () -> (); one function of it.
    module.extend([0x01, 0x04, 0x01, 0x60, 0x00, 0x00]);
    module.extend([0x03, 0x02, 0x01, 0x00]);
    // Exported as canister_init.
    module.extend([0x07, 0x11, 0x01, 0x0d]);
    module.extend(b"canister_init");
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

fn creator() -> Principal {
    user(41)
}

fn claim_key() -> SigningKey {
    SigningKey::from_bytes(&[1; 32])
}

fn cycles_ledger() -> Principal {
    Principal::from_text("um5iw-rqaaa-aaaaq-qaaba-cai").unwrap()
}

struct Net {
    pic: PocketIc,
    channel: Principal,
    factory: Principal,
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

fn factory_config(channel: Principal, seeds: [u8; 3]) -> FactoryConfig {
    FactoryConfig {
        release_channel: channel,
        release_signers: seeds.iter().map(|&seed| public_key(seed)).collect(),
        release_threshold: 2,
    }
}

/// The IC's cycles ledger and cycles minting canister, a release channel
/// that publishes what two of the keys 1, 2 and 3 signed, nothing published
/// yet, and a factory that pins the same keys. The creator holds 100T on the
/// cycles ledger.
fn net() -> Net {
    let feature = Some(IcpFeaturesConfig::DefaultConfig);
    let pic = PocketIcBuilder::new()
        .with_icp_features(IcpFeatures {
            registry: feature.clone(),
            cycles_minting: feature.clone(),
            cycles_token: feature,
            ..Default::default()
        })
        .with_application_subnet()
        .build();
    let app = pic.topology().get_app_subnets()[0];

    let channel = pic.create_canister_on_subnet(None, None, app);
    pic.add_cycles(channel, 10 * T);
    let config = ChannelConfig {
        publishers: vec![publisher()],
        release_signers: vec![public_key(1), public_key(2), public_key(3)],
        release_threshold: 2,
    };
    pic.install_canister(
        channel,
        channel_wasm(),
        encode_one(ChannelArg::Init(config)).unwrap(),
        None,
    );

    let factory = pic.create_canister_on_subnet(None, None, app);
    pic.add_cycles(factory, 10 * T);
    pic.install_canister(
        factory,
        factory_wasm(),
        encode_one(FactoryArg::Init(factory_config(channel, [1, 2, 3]))).unwrap(),
        None,
    );

    let net = Net {
        pic,
        channel,
        factory,
    };
    let sent: Result<Nat, TransferError> = update(
        &net.pic,
        cycles_ledger(),
        Principal::anonymous(),
        "icrc1_transfer",
        encode_one(TransferArg {
            from_subaccount: None,
            to: Account::from(creator()),
            fee: None,
            created_at_time: None,
            memo: None,
            amount: Nat::from(100 * T),
        })
        .unwrap(),
    );
    sent.unwrap();
    net
}

impl Net {
    /// Publishes `wasm` as `version`, tested to upgrade from `tested_from`
    /// and signed by `seeds`.
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

    fn balance(&self, owner: Principal) -> u128 {
        let balance: Nat = query(
            &self.pic,
            cycles_ledger(),
            owner,
            "icrc1_balance_of",
            encode_one(Account::from(owner)).unwrap(),
        );
        u128::try_from(balance.0).unwrap()
    }

    /// `owner` lets `factory` take up to `amount`.
    fn approve(&self, owner: Principal, factory: Principal, amount: u128) {
        let approved: Result<Nat, ApproveError> = update(
            &self.pic,
            cycles_ledger(),
            owner,
            "icrc2_approve",
            encode_one(ApproveArgs {
                from_subaccount: None,
                spender: Account::from(factory),
                amount: Nat::from(amount),
                expected_allowance: None,
                expires_at: None,
                fee: None,
                memo: None,
                created_at_time: None,
            })
            .unwrap(),
        );
        approved.unwrap();
    }

    fn args(&self, version: Option<&str>, cycles: u128) -> CreateWalletArgs {
        CreateWalletArgs {
            claim_public_key: ByteBuf::from(claim_key().verifying_key().to_bytes()),
            claim_expires_at_ns: self.pic.get_time().as_nanos_since_unix_epoch() + DAY_NS,
            version: version.map(str::to_string),
            cycles: Nat::from(cycles),
            from_subaccount: None,
        }
    }

    fn create(
        &self,
        factory: Principal,
        caller: Principal,
        args: &CreateWalletArgs,
    ) -> Result<CreatedWallet, FactoryError> {
        update(
            &self.pic,
            factory,
            caller,
            "create_wallet",
            encode_one(args).unwrap(),
        )
    }

    fn current_release(&self, factory: Principal) -> Result<ReleaseManifest, FactoryError> {
        update(
            &self.pic,
            factory,
            Principal::anonymous(),
            "current_release",
            encode_one(()).unwrap(),
        )
    }

    fn module_hash(&self, canister: Principal) -> Option<Vec<u8>> {
        let controller = self.pic.get_controllers(canister)[0];
        self.pic
            .canister_status(canister, Some(controller))
            .unwrap()
            .module_hash
    }

    fn stored_chunks(&self, canister: Principal) -> usize {
        let controller = self.pic.get_controllers(canister)[0];
        self.pic
            .stored_chunks(canister, Some(controller))
            .unwrap()
            .len()
    }

    fn status(&self, wallet: Principal) -> Status {
        query(
            &self.pic,
            wallet,
            Principal::anonymous(),
            "status",
            encode_one(()).unwrap(),
        )
    }

    /// `owner` claims `wallet` with the claim key.
    fn claim(&self, wallet: Principal, owner: Principal) -> Result<Member, WalletError> {
        let message = wallet::claim_message(wallet, owner);
        let signature = ByteBuf::from(claim_key().sign(message.as_bytes()).to_bytes());
        update(
            &self.pic,
            wallet,
            owner,
            "claim",
            encode_one(ClaimArgs { signature }).unwrap(),
        )
    }

    /// The factory's own balance.
    fn factory_cycles(&self) -> u128 {
        self.pic.cycle_balance(self.factory)
    }
}

/// A wallet the factory just created belongs to itself alone: it runs the
/// release, holds the cycles the creator paid less the IC's fees, keeps no
/// chunk, can be claimed, and upgrades itself.
fn assert_self_owned(net: &Net, created: &CreatedWallet, release: &ReleaseManifest) {
    let wallet = created.wallet;
    assert_eq!(net.pic.get_controllers(wallet), vec![wallet]);
    assert_eq!(
        net.module_hash(wallet).as_deref(),
        Some(&release.wasm_sha256[..])
    );
    assert_eq!(net.stored_chunks(wallet), 0);
    assert_eq!(created.version, release.version);
    let status = net.status(wallet);
    assert!(!status.claimed);
    assert_eq!(status.release_channel, net.channel);
    assert_eq!(status.upgrade, UpgradeState::Idle);
}

#[test]
fn the_factory_creates_a_wallet_that_owns_itself_and_upgrades_itself() {
    let net = net();
    let first = wallet_wasm();
    let release = net.publish("0.1.0", &first, vec![], &[1, 2]);
    assert!(
        release.chunk_hashes.len() > 1,
        "a release of several chunks"
    );
    assert_eq!(net.current_release(net.factory), Ok(release.clone()));

    let paid = 3 * T;
    net.approve(creator(), net.factory, paid + FEE);
    let before = net.balance(creator());
    let factory_before = net.factory_cycles();
    let args = net.args(None, paid);
    let created = net.create(net.factory, creator(), &args).unwrap();
    let factory_spent = factory_before - net.factory_cycles();

    assert_self_owned(&net, &created, &release);
    assert_eq!(created.cycles, Nat::from(paid));
    assert_eq!(net.balance(creator()), before - paid - FEE);
    let status = net.status(created.wallet);
    assert_eq!(status.claim_expires_at_ns, Some(args.claim_expires_at_ns));
    let left = u128::try_from(status.cycles.0).unwrap();
    // 0.5T to create the canister on a 13-node subnet, and the install.
    assert!(left < paid - T / 2 && left > paid - T, "{left}");
    println!(
        "cost: of {paid} paid, the wallet starts with {left}; the factory spent {factory_spent} on it"
    );

    let owner = user(7);
    let member = net.claim(created.wallet, owner).unwrap();
    assert_eq!((member.id, member.role), (owner, Role::Owner));

    // The wallet controls itself, so it installs the next release itself.
    let next_wasm = release_of(&first, "0.2.0");
    let next = net.publish("0.2.0", &next_wasm, vec![sha256(&first)], &[2, 3]);
    let proposed: Result<wallet::Pending, WalletError> = update(
        &net.pic,
        created.wallet,
        owner,
        "propose",
        encode_one(Action::Upgrade {
            version: next.version.clone(),
            wasm_sha256: next.wasm_sha256.clone(),
        })
        .unwrap(),
    );
    let id = proposed.unwrap().id;
    net.pic.advance_time(Duration::from_nanos(7 * DAY_NS + 1));
    let sent: Result<wallet::Pending, WalletError> = update(
        &net.pic,
        created.wallet,
        owner,
        "execute",
        encode_one(id).unwrap(),
    );
    assert_eq!(sent.unwrap().state, ActionState::Ready);
    for _ in 0..4 {
        net.pic.tick();
    }
    assert_eq!(
        net.module_hash(created.wallet),
        Some(sha256(&next_wasm).to_vec())
    );
    let pending: Result<Vec<wallet::Pending>, WalletError> = query(
        &net.pic,
        created.wallet,
        owner,
        "pending",
        encode_one(()).unwrap(),
    );
    assert!(pending.unwrap().is_empty());
    let executed: Result<wallet::Pending, WalletError> = update(
        &net.pic,
        created.wallet,
        owner,
        "execute",
        encode_one(id).unwrap(),
    );
    assert_eq!(
        executed,
        Err(WalletError::InvalidArgument(format!(
            "action {id} is already executed"
        )))
    );

    // A wallet created now gets the latest release, or the one asked for.
    net.approve(creator(), net.factory, 2 * (MIN_CYCLES + FEE));
    let latest = net
        .create(net.factory, creator(), &net.args(None, MIN_CYCLES))
        .unwrap();
    assert_self_owned(&net, &latest, &next);
    let older = net
        .create(net.factory, creator(), &net.args(Some("0.1.0"), MIN_CYCLES))
        .unwrap();
    assert_self_owned(&net, &older, &release);
    assert_ne!(latest.wallet, older.wallet);
}

#[test]
fn the_factory_takes_nothing_for_a_wallet_it_cannot_create() {
    let net = net();
    // Nothing published yet.
    assert!(matches!(
        net.current_release(net.factory),
        Err(FactoryError::ReleaseUnavailable(reason)) if reason.contains("published no release")
    ));
    net.approve(creator(), net.factory, 2 * T + FEE);
    let before = net.balance(creator());
    assert!(matches!(
        net.create(net.factory, creator(), &net.args(None, 2 * T)),
        Err(FactoryError::ReleaseUnavailable(reason)) if reason.contains("published no release")
    ));
    let release = net.publish("0.1.0", &wallet_wasm(), vec![], &[1, 3]);

    assert!(matches!(
        net.create(net.factory, creator(), &net.args(Some("0.9.0"), 2 * T)),
        Err(FactoryError::ReleaseUnavailable(reason)) if reason.contains("no release 0.9.0")
    ));
    // More than the allowance, and no allowance at all.
    assert_eq!(
        net.create(net.factory, creator(), &net.args(None, 3 * T)),
        Err(FactoryError::InsufficientAllowance {
            allowance: Nat::from(2 * T + FEE)
        })
    );
    assert_eq!(
        net.create(net.factory, user(42), &net.args(None, 2 * T)),
        Err(FactoryError::InsufficientAllowance {
            allowance: Nat::from(0u8)
        })
    );
    // An allowance without the cycles behind it.
    let poor = user(43);
    let sent: Result<Nat, TransferError> = update(
        &net.pic,
        cycles_ledger(),
        Principal::anonymous(),
        "icrc1_transfer",
        encode_one(TransferArg {
            from_subaccount: None,
            to: Account::from(poor),
            fee: None,
            created_at_time: None,
            memo: None,
            amount: Nat::from(T),
        })
        .unwrap(),
    );
    sent.unwrap();
    net.approve(poor, net.factory, 2 * T + FEE);
    assert_eq!(
        net.create(net.factory, poor, &net.args(None, 2 * T)),
        Err(FactoryError::InsufficientFunds {
            balance: Nat::from(T - FEE)
        })
    );

    // Arguments out of form are refused before anything else.
    let refusals: Vec<(Principal, CreateWalletArgs, &str)> = vec![
        (Principal::anonymous(), net.args(None, 2 * T), "anonymous"),
        (
            creator(),
            CreateWalletArgs {
                claim_expires_at_ns: net.pic.get_time().as_nanos_since_unix_epoch() + HOUR_NS / 2,
                ..net.args(None, 2 * T)
            },
            "an hour from now",
        ),
        (creator(), net.args(None, MIN_CYCLES - 1), "cycles"),
        (
            creator(),
            CreateWalletArgs {
                claim_public_key: ByteBuf::from(vec![1; 31]),
                ..net.args(None, 2 * T)
            },
            "claim_public_key",
        ),
    ];
    for (caller, args, refusal) in refusals {
        assert!(
            matches!(
                net.create(net.factory, caller, &args),
                Err(FactoryError::InvalidArgument(reason)) if reason.contains(refusal)
            ),
            "{refusal}"
        );
    }

    // A factory that pins other keys refuses the release before anyone pays.
    let app = net.pic.topology().get_app_subnets()[0];
    let strict = net.pic.create_canister_on_subnet(None, None, app);
    net.pic.add_cycles(strict, 10 * T);
    net.pic.install_canister(
        strict,
        factory_wasm(),
        encode_one(FactoryArg::Init(factory_config(net.channel, [3, 4, 5]))).unwrap(),
        None,
    );
    let one_of_two = FactoryError::ReleaseRejected(format!(
        "release {} has 1 of the 2 signatures it needs",
        release.version
    ));
    assert_eq!(net.current_release(strict), Err(one_of_two.clone()));
    net.approve(creator(), strict, 2 * T + FEE);
    let before_strict = net.balance(creator());
    assert_eq!(
        net.create(strict, creator(), &net.args(None, 2 * T)),
        Err(one_of_two)
    );
    // Only the approvals were paid for.
    assert_eq!(net.balance(creator()), before - FEE);
    assert_eq!(before_strict, before - FEE);
}

#[test]
fn a_release_that_does_not_install_leaves_the_canister_to_its_creator() {
    let net = net();
    let broken = trapping_init_module();
    let release = net.publish("0.1.0-broken", &broken, vec![], &[1, 2]);
    let paid = 2 * T;
    net.approve(creator(), net.factory, paid + FEE);
    let before = net.balance(creator());
    let failed = net.create(net.factory, creator(), &net.args(None, paid));
    let Err(FactoryError::InstallFailed { wallet, reason }) = failed else {
        panic!("{failed:?}");
    };
    assert!(reason.contains("install_chunked_code"), "{reason}");
    assert_eq!(net.balance(creator()), before - paid - FEE);
    assert_eq!(net.pic.get_controllers(wallet), vec![creator()]);
    assert_eq!(net.module_hash(wallet), None);
    assert_eq!(net.stored_chunks(wallet), 0);
    assert!(net.pic.cycle_balance(wallet) > paid - T);
    // The creator can install the release itself, now that it holds the
    // canister: the factory kept no rights over it.
    assert_eq!(sha256(&broken).to_vec(), release.wasm_sha256.to_vec());
}

/// A hand-over that fails stays on record, and the IC's timer finishes it:
/// the factory is frozen from the moment the wallet runs until its
/// `create_wallet` returns, so it cannot make the calls that hand over.
#[test]
fn the_timer_hands_over_a_wallet_whose_hand_over_failed() {
    let net = net();
    let release = net.publish("0.1.0", &wallet_wasm(), vec![], &[1, 2]);
    let paid = 2 * T;
    net.approve(creator(), net.factory, paid + FEE);
    let args = net.args(None, paid);
    let message = net
        .pic
        .submit_call(
            net.factory,
            creator(),
            "create_wallet",
            encode_one(&args).unwrap(),
        )
        .unwrap();
    // Round by round, until the release runs in the new wallet: the cycles
    // minting canister gives it the next id on the subnet.
    let next = next_canister(net.factory);
    let mut wallet = None;
    for _ in 0..200 {
        net.pic.tick();
        if net.pic.canister_exists(next)
            && net.module_hash(next).as_deref() == Some(&release.wasm_sha256[..])
        {
            wallet = Some(next);
            break;
        }
    }
    let wallet = wallet.expect("the release runs in a new wallet");
    let freeze = |threshold: u64| {
        net.pic
            .update_canister_settings(
                net.factory,
                None,
                CanisterSettings {
                    freezing_threshold: Some(Nat::from(threshold)),
                    ..Default::default()
                },
            )
            .unwrap();
    };
    freeze(u64::MAX / 4);
    let reply = net.pic.await_call(message).unwrap();
    let created: Result<CreatedWallet, FactoryError> = decode_one(&reply).unwrap();
    assert_eq!(created.unwrap().wallet, wallet);
    assert!(net.pic.get_controllers(wallet).contains(&net.factory));

    // Thawed, the factory hands the wallet over at its next timer, five
    // minutes on, though another wallet was created in between.
    freeze(30 * 24 * 60 * 60);
    net.pic.advance_time(Duration::from_secs(4 * 60));
    net.approve(creator(), net.factory, MIN_CYCLES + FEE);
    let other = net
        .create(net.factory, creator(), &net.args(None, MIN_CYCLES))
        .unwrap();
    assert_self_owned(&net, &other, &release);
    assert!(net.pic.get_controllers(wallet).contains(&net.factory));
    net.pic.advance_time(Duration::from_secs(60));
    for _ in 0..10 {
        net.pic.tick();
    }
    assert_eq!(net.pic.get_controllers(wallet), vec![wallet]);
    assert_eq!(net.stored_chunks(wallet), 0);
}

/// The canister id after `id` on its subnet.
fn next_canister(id: Principal) -> Principal {
    let bytes = id.as_slice();
    let index = u64::from_be_bytes(bytes[..8].try_into().unwrap());
    let mut next = (index + 1).to_be_bytes().to_vec();
    next.extend_from_slice(&bytes[8..]);
    Principal::from_slice(&next)
}

#[test]
fn an_upgrade_keeps_the_factory_s_configuration_or_replaces_it() {
    let net = net();
    let release = net.publish("0.1.0", &wallet_wasm(), vec![], &[1, 2]);
    assert_eq!(net.current_release(net.factory), Ok(release.clone()));
    net.pic
        .upgrade_canister(
            net.factory,
            factory_wasm(),
            encode_one(FactoryArg::Upgrade(None)).unwrap(),
            None,
        )
        .unwrap();
    assert_eq!(net.current_release(net.factory), Ok(release.clone()));
    net.pic
        .upgrade_canister(
            net.factory,
            factory_wasm(),
            encode_one(FactoryArg::Upgrade(Some(factory_config(
                net.channel,
                [3, 4, 5],
            ))))
            .unwrap(),
            None,
        )
        .unwrap();
    assert!(matches!(
        net.current_release(net.factory),
        Err(FactoryError::ReleaseRejected(_))
    ));
    // An install argument, or a configuration out of form, is refused.
    assert!(
        net.pic
            .upgrade_canister(
                net.factory,
                factory_wasm(),
                encode_one(FactoryArg::Init(factory_config(net.channel, [1, 2, 3]))).unwrap(),
                None,
            )
            .is_err()
    );
    assert!(
        net.pic
            .upgrade_canister(
                net.factory,
                factory_wasm(),
                encode_one(FactoryArg::Upgrade(Some(factory_config(
                    net.channel,
                    [1, 1, 2],
                ))))
                .unwrap(),
                None,
            )
            .is_err()
    );
}
