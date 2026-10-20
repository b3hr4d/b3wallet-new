//! Firmware of a self-owned chain-key wallet canister.
//!
//! Step 1 of `docs/plan-v0.md`: the install arguments and `status`. The
//! interface test lists the methods of `candid/wallet.did` not implemented
//! yet.

mod types;

pub use types::*;

use candid::Nat;
use std::cell::RefCell;

thread_local! {
    // Heap only until step 2 moves it to stable memory, which is why
    // `post_upgrade` refuses every upgrade for now.
    static CONFIG: RefCell<Option<InitArgs>> = const { RefCell::new(None) };
}

#[ic_cdk::init]
fn init(arg: WalletArg) {
    let WalletArg::Init(args) = arg else {
        ic_cdk::trap("install needs WalletArg::Init");
    };
    if let Err(reason) = args.validate(ic_cdk::api::time()) {
        ic_cdk::trap(format!("invalid InitArgs: {reason}"));
    }
    CONFIG.with_borrow_mut(|config| *config = Some(args));
}

// Trapping here makes the IC keep the running version, so an upgrade cannot
// silently drop the heap state above.
#[ic_cdk::post_upgrade]
fn post_upgrade(_arg: WalletArg) {
    ic_cdk::trap("upgrades are refused until the state lives in stable memory");
}

#[ic_cdk::query]
fn status() -> Status {
    CONFIG.with_borrow(|config| {
        let Some(config) = config else {
            ic_cdk::trap("the wallet was installed without InitArgs");
        };
        Status {
            version: env!("CARGO_PKG_VERSION").to_string(),
            claimed: false,
            claim_expires_at_ns: Some(config.claim_expires_at_ns),
            cycles: Nat::from(ic_cdk::api::canister_cycle_balance()),
            upgrade: UpgradeState::Idle,
            release_channel: config.release_channel,
        }
    })
}

ic_cdk::export_candid!();

#[cfg(test)]
mod interface {
    use candid::TypeEnv;
    use candid::types::subtype::equal;
    use candid::types::{Type, TypeInner};
    use candid_parser::utils::CandidSource;
    use std::collections::HashSet;
    use std::path::Path;

    const TARGET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../candid/wallet.did");

    fn class(env: &TypeEnv, actor: &Type) -> (Vec<Type>, Vec<(String, Type)>) {
        let actor = env.trace_type(actor).expect("actor type");
        let TypeInner::Class(args, service) = actor.as_ref() else {
            panic!("the interface has no init arguments: {actor}");
        };
        let methods = env.as_service(service).expect("service type").to_vec();
        (args.clone(), methods)
    }

    /// Every method the canister exports must exist in `candid/wallet.did`
    /// with an equal type, mode included, and the init arguments must match.
    /// Methods not implemented yet are printed, not failed.
    #[test]
    fn implemented_methods_match_the_target_interface() {
        let exported = super::__export_service();
        let (mut env, exported_actor) = CandidSource::Text(&exported)
            .load()
            .expect("exported .did parses");
        let (target_env, target_actor) = CandidSource::File(Path::new(TARGET))
            .load()
            .expect("candid/wallet.did parses");
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

        let mut mismatches = Vec::new();
        for (name, exported) in &exported_methods {
            match target_methods
                .iter()
                .find(|(target_name, _)| target_name == name)
            {
                None => mismatches.push(format!("{name}: not in candid/wallet.did")),
                Some((_, target)) => {
                    if let Err(e) = equal(&mut gamma, &env, exported, target) {
                        mismatches.push(format!("{name}: {exported} differs from {target}: {e}"));
                    }
                }
            }
        }
        assert!(
            mismatches.is_empty(),
            "interface mismatches:\n{}",
            mismatches.join("\n")
        );

        let missing: Vec<&str> = target_methods
            .iter()
            .map(|(name, _)| name.as_str())
            .filter(|name| {
                !exported_methods
                    .iter()
                    .any(|(exported, _)| exported == name)
            })
            .collect();
        eprintln!(
            "implemented {} of {} methods; not yet: {}",
            exported_methods.len(),
            target_methods.len(),
            missing.join(", ")
        );
    }
}
