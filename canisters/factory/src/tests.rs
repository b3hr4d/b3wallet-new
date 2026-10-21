use crate::storage::HandOver;
use crate::types::*;
use crate::{Busy, next_retry};
use candid::{Nat, Principal};
use ed25519_dalek::SigningKey;
use serde_bytes::ByteBuf;

const NOW: u64 = 1_000_000_000_000_000_000;

fn key(seed: u8) -> ByteBuf {
    ByteBuf::from(
        SigningKey::from_bytes(&[seed; 32])
            .verifying_key()
            .to_bytes(),
    )
}

fn channel() -> Principal {
    Principal::from_slice(&[0, 0, 0, 0, 0, 0, 0, 7, 1, 1])
}

fn config() -> FactoryConfig {
    FactoryConfig {
        release_channel: channel(),
        release_signers: vec![key(1), key(2), key(3)],
        release_threshold: 2,
    }
}

fn creator() -> Principal {
    Principal::self_authenticating([9; 32])
}

fn args() -> CreateWalletArgs {
    CreateWalletArgs {
        claim_public_key: key(7),
        claim_expires_at_ns: NOW + MIN_CLAIM_WINDOW_NS,
        version: Some("1.0.0".to_string()),
        cycles: Nat::from(MIN_CYCLES),
        from_subaccount: Some(ByteBuf::from(vec![0; 32])),
    }
}

/// A change that makes a valid value invalid.
type Change<T> = fn(&mut T);

#[test]
fn a_two_of_three_configuration_is_valid() {
    assert_eq!(config().validate().unwrap().threshold(), 2);
}

#[test]
fn the_configuration_refuses_a_channel_that_is_no_canister_and_bad_signers() {
    let changes: Vec<(Change<FactoryConfig>, &str)> = vec![
        (|c| c.release_channel = creator(), "canister id"),
        (|c| c.release_signers.clear(), "threshold"),
        (|c| c.release_signers.push(key(1)), "repeat"),
        (|c| c.release_threshold = 0, "threshold"),
        (|c| c.release_threshold = 4, "threshold"),
        (
            |c| c.release_signers = (1..=17).map(key).collect(),
            "16 release signers",
        ),
    ];
    for (change, refusal) in changes {
        let mut changed = config();
        change(&mut changed);
        let reason = changed.validate().expect_err(refusal);
        assert!(reason.contains(refusal), "{reason} names {refusal}");
    }
}

#[test]
fn valid_arguments_pay_the_cycles_they_name() {
    assert_eq!(args().validate(creator(), NOW), Ok(MIN_CYCLES));
    let no_version_or_subaccount = CreateWalletArgs {
        version: None,
        from_subaccount: None,
        ..args()
    };
    assert_eq!(
        no_version_or_subaccount.validate(creator(), NOW),
        Ok(MIN_CYCLES)
    );
}

#[test]
fn the_arguments_refuse_each_field_out_of_form() {
    let changes: Vec<(Change<CreateWalletArgs>, &str)> = vec![
        (
            |a| a.claim_public_key = ByteBuf::from(vec![7; 31]),
            "claim_public_key",
        ),
        // The identity point is a weak key: a signature by it proves nothing.
        (
            |a| {
                let mut identity = vec![0; 32];
                identity[0] = 1;
                a.claim_public_key = ByteBuf::from(identity);
            },
            "claim_public_key",
        ),
        (
            |a| a.claim_expires_at_ns = NOW + MIN_CLAIM_WINDOW_NS - 1,
            "an hour from now",
        ),
        (|a| a.claim_expires_at_ns = 0, "an hour from now"),
        (|a| a.version = Some(String::new()), "a version"),
        (|a| a.version = Some("1.0 beta".to_string()), "a version"),
        (
            |a| a.from_subaccount = Some(ByteBuf::from(vec![0; 31])),
            "from_subaccount",
        ),
        (|a| a.cycles = Nat::from(MIN_CYCLES - 1), "cycles"),
        (|a| a.cycles = Nat::from(u128::MAX) + 1u8, "128 bits"),
    ];
    for (change, refusal) in changes {
        let mut changed = args();
        change(&mut changed);
        let reason = changed.validate(creator(), NOW).expect_err(refusal);
        assert!(reason.contains(refusal), "{reason} names {refusal}");
    }
    let reason = args()
        .validate(Principal::anonymous(), NOW)
        .expect_err("anonymous");
    assert!(reason.contains("anonymous"), "{reason}");
}

#[test]
fn a_canister_that_runs_its_release_goes_to_itself_and_any_other_to_its_creator() {
    let canister = Principal::from_slice(&[0, 0, 0, 0, 0, 0, 0, 9, 1, 1]);
    let record = HandOver {
        creator: creator(),
        wasm_sha256: ByteBuf::from(vec![5; 32]),
        created_at_ns: NOW,
    };
    assert_eq!(crate::owner_of(canister, &record, Some(&[5; 32])), canister);
    assert_eq!(crate::owner_of(canister, &record, None), creator());
    assert_eq!(
        crate::owner_of(canister, &record, Some(&[6; 32])),
        creator()
    );
}

#[test]
fn the_cycles_ledger_is_the_one_on_mainnet() {
    assert_eq!(
        crate::cycles_ledger::id().to_text(),
        "um5iw-rqaaa-aaaaq-qaaba-cai"
    );
}

#[test]
fn a_canister_busy_elsewhere_stays_busy() {
    let canister = Principal::from_slice(&[0, 0, 0, 0, 0, 0, 0, 9, 1, 1]);
    let creation = Busy::start(canister);
    assert!(Busy::try_start(canister).is_none());
    // The refused attempt left the creation's mark.
    assert!(Busy::try_start(canister).is_none());
    drop(creation);
    assert!(Busy::try_start(canister).is_some());
}

#[test]
fn a_retry_already_set_keeps_its_deadline() {
    assert_eq!(next_retry(0, NOW), NOW);
    assert_eq!(next_retry(NOW - 1, NOW), NOW - 1);
    assert_eq!(next_retry(NOW + 1, NOW), NOW);
}

mod interface {
    use candid::TypeEnv;
    use candid::types::internal::TypeContainer;
    use candid::types::subtype::equal;
    use candid::types::{Type, TypeInner};
    use candid_parser::utils::CandidSource;
    use std::collections::HashSet;
    use std::path::Path;

    fn did(name: &str) -> String {
        format!("{}/../../candid/{name}", env!("CARGO_MANIFEST_DIR"))
    }

    fn class(env: &TypeEnv, actor: &Type) -> (Vec<Type>, Vec<(String, Type)>) {
        let actor = env.trace_type(actor).expect("actor type");
        let TypeInner::Class(args, service) = actor.as_ref() else {
            panic!("the interface has no init arguments: {actor}");
        };
        let methods = env.as_service(service).expect("service type").to_vec();
        (args.clone(), methods)
    }

    /// The canister exports exactly the methods of `candid/factory.did`, each
    /// with an equal type, mode included, and the same init arguments.
    #[test]
    fn the_canister_implements_the_target_interface() {
        let exported = crate::__export_service();
        let (mut env, exported_actor) = CandidSource::Text(&exported)
            .load()
            .expect("exported .did parses");
        let target = did("factory.did");
        let (target_env, target_actor) = CandidSource::File(Path::new(&target))
            .load()
            .expect("candid/factory.did parses");
        let target_actor = env.merge_type(target_env, target_actor.expect("target service"));
        let (exported_args, exported_methods) =
            class(&env, &exported_actor.expect("exported service"));
        let (target_args, target_methods) = class(&env, &target_actor);

        let mut gamma = HashSet::new();
        assert_eq!(
            exported_args.len(),
            target_args.len(),
            "init argument count"
        );
        for (exported, target) in exported_args.iter().zip(&target_args) {
            equal(&mut gamma, &env, exported, target)
                .unwrap_or_else(|e| panic!("init argument {exported} differs from {target}: {e}"));
        }
        let names = |methods: &[(String, Type)]| -> Vec<String> {
            let mut names: Vec<String> = methods.iter().map(|(name, _)| name.clone()).collect();
            names.sort();
            names
        };
        assert_eq!(names(&exported_methods), names(&target_methods));
        for (name, exported) in &exported_methods {
            let (_, target) = target_methods
                .iter()
                .find(|(target_name, _)| target_name == name)
                .expect("the names match");
            equal(&mut gamma, &env, exported, target)
                .unwrap_or_else(|e| panic!("{name}: {exported} differs from {target}: {e}"));
        }
    }

    /// The factory installs wallets with the wallet's own install argument.
    #[test]
    fn the_wallet_install_argument_is_the_wallets() {
        let mut container = TypeContainer::new();
        let sent = container.add::<crate::WalletArg>();
        let mut env = container.env;
        let wallet = did("wallet.did");
        let (wallet_env, _) = CandidSource::File(Path::new(&wallet))
            .load()
            .expect("candid/wallet.did parses");
        let target = env.merge_type(wallet_env, TypeInner::Var("WalletArg".to_string()).into());
        equal(&mut HashSet::new(), &env, &sent, &target)
            .unwrap_or_else(|e| panic!("{sent} differs from the wallet's WalletArg: {e}"));
    }
}
