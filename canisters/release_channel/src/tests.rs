use candid::Principal;
use release::channel::ChannelConfig;
use serde_bytes::ByteBuf;

fn signer(seed: u8) -> ByteBuf {
    use ed25519_dalek::SigningKey;
    ByteBuf::from(
        SigningKey::from_bytes(&[seed; 32])
            .verifying_key()
            .to_bytes(),
    )
}

fn config() -> ChannelConfig {
    ChannelConfig {
        publishers: vec![Principal::from_slice(&[1; 29])],
        release_signers: vec![signer(1), signer(2), signer(3)],
        release_threshold: 2,
    }
}

#[test]
fn a_two_of_three_configuration_is_valid() {
    let signers = config().validate().unwrap();
    assert_eq!(signers.threshold(), 2);
}

#[test]
fn the_configuration_refuses_bad_publishers_and_signers() {
    let changes: Vec<(Change<ChannelConfig>, &str)> = vec![
        (|c| c.publishers.push(Principal::anonymous()), "anonymous"),
        (
            |c| c.publishers = vec![Principal::from_slice(&[1; 29]); 17],
            "publishers",
        ),
        (|c| c.release_signers.clear(), "threshold"),
        (|c| c.release_signers.push(signer(1)), "repeat"),
        (
            |c| c.release_signers.push(ByteBuf::from(vec![2; 32])),
            "key 3",
        ),
        (|c| c.release_threshold = 0, "threshold"),
        (|c| c.release_threshold = 4, "threshold"),
        (
            |c| c.release_signers = (1..=17).map(signer).collect(),
            "16 release signers",
        ),
    ];
    for (change, refusal) in changes {
        let mut changed = config();
        change(&mut changed);
        let reason = changed.validate().expect_err(refusal);
        assert!(reason.contains(refusal), "{reason} names {refusal}");
    }
    // A channel nobody publishes to keeps serving what it has.
    let mut frozen = config();
    frozen.publishers.clear();
    assert!(frozen.validate().is_ok());
}

mod interface {
    use candid::TypeEnv;
    use candid::types::subtype::equal;
    use candid::types::{Type, TypeInner};
    use candid_parser::utils::CandidSource;
    use std::collections::HashSet;
    use std::path::Path;

    const TARGET: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../candid/release_channel.did"
    );

    fn class(env: &TypeEnv, actor: &Type) -> (Vec<Type>, Vec<(String, Type)>) {
        let actor = env.trace_type(actor).expect("actor type");
        let TypeInner::Class(args, service) = actor.as_ref() else {
            panic!("the interface has no init arguments: {actor}");
        };
        let methods = env.as_service(service).expect("service type").to_vec();
        (args.clone(), methods)
    }

    /// The canister exports exactly the methods of
    /// `candid/release_channel.did`, each with an equal type, mode included,
    /// and the same init arguments.
    #[test]
    fn the_canister_implements_the_target_interface() {
        let exported = crate::__export_service();
        let (mut env, exported_actor) = CandidSource::Text(&exported)
            .load()
            .expect("exported .did parses");
        let (target_env, target_actor) = CandidSource::File(Path::new(TARGET))
            .load()
            .expect("candid/release_channel.did parses");
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
}

/// A change that makes a valid value invalid.
type Change<T> = fn(&mut T);
