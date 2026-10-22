use super::*;
use ed25519_dalek::{Signer, SigningKey};
use proptest::prelude::*;

const T0: u64 = 1_000 * DAY_NS;

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn public(seed: u8) -> ByteBuf {
    ByteBuf::from(key(seed).verifying_key().to_bytes())
}

fn controller() -> Principal {
    Principal::from_slice(&[0, 0, 0, 0, 0, 0, 0, 1, 1, 1])
}

fn wallet() -> Principal {
    Principal::from_slice(&[0, 0, 0, 0, 0, 0, 0, 2, 1, 1])
}

/// Three recovery keys, 1 of them and 48 hours for a signed release, 2 and
/// 7 days for anything else, and release signers 21, 22 and 23, two of which
/// must sign.
fn policy() -> Policy {
    Policy {
        authorities: (1..=3)
            .map(|seed| Authority {
                public_key: public(seed),
                label: format!("key {seed}"),
            })
            .collect(),
        normal_threshold: 1,
        sovereign_threshold: 2,
        normal_delay_ns: 48 * HOUR_NS,
        sovereign_delay_ns: 7 * DAY_NS,
        release_signers: (21..=23).map(public).collect(),
        release_threshold: 2,
    }
}

fn proof(seed: u8, controller: Principal, wallet: Principal) -> Proof {
    let key = key(seed);
    let public_key = key.verifying_key().to_bytes();
    Proof {
        public_key: ByteBuf::from(public_key),
        signature: ByteBuf::from(
            key.sign(&enroll_message(controller, wallet, &public_key))
                .to_bytes(),
        ),
    }
}

fn wasm() -> Vec<u8> {
    (0..release::CHUNK_SIZE + 10).map(|i| i as u8).collect()
}

fn signed_release(wasm: &[u8], signers: &[u8]) -> ReleaseManifest {
    let mut manifest = ReleaseManifest::new(
        "1.2.0".to_string(),
        wasm,
        vec![[5; 32]],
        "https://example.org/1.2.0".to_string(),
        T0,
    );
    for &seed in signers {
        manifest.sign(&key(seed));
    }
    manifest
}

fn install(release: Option<ReleaseManifest>) -> Operation {
    let wasm = wasm();
    Operation {
        controller: controller(),
        wallet: wallet(),
        policy_version: 1,
        nonce: 0,
        expires_at_ns: T0 + 10 * DAY_NS,
        kind: OperationKind::InstallCode(InstallCode {
            mode: InstallMode::Upgrade {
                skip_pre_upgrade: false,
            },
            wasm_sha256: ByteBuf::from(sha256(&wasm)),
            chunk_hashes: release::chunks(&wasm)
                .map(|chunk| ByteBuf::from(sha256(chunk)))
                .collect(),
            arg: ByteBuf::from(b"DIDL\x00\x00".to_vec()),
            release,
        }),
    }
}

fn context(policy: &Policy) -> Context<'_> {
    Context {
        controller: controller(),
        wallet: wallet(),
        policy,
        policy_version: 1,
        next_nonce: 0,
        now: T0,
    }
}

fn with_policy(change: impl FnOnce(&mut Policy)) -> Policy {
    let mut policy = policy();
    change(&mut policy);
    policy
}

#[test]
fn the_default_policy_passes() {
    assert_eq!(policy().validate(), Ok(()));
}

#[test]
fn a_policy_needs_keys_that_are_distinct_valid_and_few() {
    let none = with_policy(|p| p.authorities.clear());
    assert!(none.validate().unwrap_err().contains("3 to 10"));

    let eleven = with_policy(|p| {
        p.authorities = (1..=11)
            .map(|seed| Authority {
                public_key: public(seed),
                label: format!("key {seed}"),
            })
            .collect();
    });
    assert!(eleven.validate().unwrap_err().contains("3 to 10"));

    let twice = with_policy(|p| p.authorities[2].public_key = public(1));
    assert!(twice.validate().unwrap_err().contains("listed twice"));

    let short = with_policy(|p| p.authorities[0].public_key = ByteBuf::from(vec![1; 31]));
    assert!(short.validate().unwrap_err().contains("not an Ed25519"));

    // The identity point is a weak key: any signature verifies for it.
    let mut identity = [0u8; 32];
    identity[0] = 1;
    let weak = with_policy(|p| p.authorities[0].public_key = ByteBuf::from(identity.to_vec()));
    assert!(weak.validate().unwrap_err().contains("weak"));

    // Labels are printable ASCII and tell the keys apart, so a label cannot
    // pass for another key's on a screen.
    let long = with_policy(|p| p.authorities[0].label = "x".repeat(65));
    assert!(long.validate().unwrap_err().contains("label"));
    let longest = with_policy(|p| p.authorities[0].label = "~".repeat(64));
    assert_eq!(longest.validate(), Ok(()));
    for label in [
        "",
        "a\nb",
        "key\u{7f}",
        " key 1",
        "key 1 ",
        "kеy 9",
        "key 9\u{202e}",
        "\u{200b}key 9",
    ] {
        let policy = with_policy(|p| p.authorities[0].label = label.to_string());
        let reason = policy.validate().unwrap_err();
        assert!(reason.contains("printable ASCII"), "{label:?}: {reason}");
    }
    let same = with_policy(|p| p.authorities[2].label = "KEY 1".to_string());
    assert!(same.validate().unwrap_err().contains("another key's label"));
}

#[test]
fn thresholds_take_two_keys_and_leave_room_for_one_lost_key() {
    let zero = with_policy(|p| p.normal_threshold = 0);
    assert!(zero.validate().is_err());
    // The sovereign route is the stronger one.
    let equal = with_policy(|p| p.normal_threshold = 2);
    assert!(
        equal
            .validate()
            .unwrap_err()
            .contains("below the sovereign")
    );
    let above = with_policy(|p| p.normal_threshold = 3);
    assert!(above.validate().is_err());
    // Two of three keys must do anything; three of three would let one lost
    // key block every repair and rotation, and one would let one stolen key
    // take the wallet.
    let all = with_policy(|p| p.sovereign_threshold = 3);
    assert!(all.validate().unwrap_err().contains("one lost key"));
    let more = with_policy(|p| p.sovereign_threshold = 4);
    assert!(more.validate().is_err());
    let one = with_policy(|p| {
        p.sovereign_threshold = 1;
        p.normal_threshold = 0;
    });
    assert!(one.validate().unwrap_err().contains("one stolen key"));

    // So one or two keys are never enough.
    for keys in [1, 2] {
        for sovereign in 1..=2 {
            let policy = with_policy(|p| {
                p.authorities.truncate(keys);
                p.sovereign_threshold = sovereign;
                p.normal_threshold = 1;
            });
            assert!(
                policy.validate().unwrap_err().contains("3 to 10"),
                "{keys} keys, {sovereign}"
            );
        }
    }
    let five = with_policy(|p| {
        p.authorities = (1..=5)
            .map(|seed| Authority {
                public_key: public(seed),
                label: format!("key {seed}"),
            })
            .collect();
        p.normal_threshold = 3;
        p.sovereign_threshold = 4;
    });
    assert_eq!(five.validate(), Ok(()));
}

#[test]
fn delays_have_floors_and_a_ceiling() {
    let short = with_policy(|p| p.normal_delay_ns = MIN_NORMAL_DELAY_NS - 1);
    assert!(short.validate().unwrap_err().contains("24 hours"));
    let sovereign = with_policy(|p| p.sovereign_delay_ns = MIN_SOVEREIGN_DELAY_NS - 1);
    assert!(sovereign.validate().unwrap_err().contains("72 hours"));
    let below_normal = with_policy(|p| {
        p.normal_delay_ns = 10 * DAY_NS;
        p.sovereign_delay_ns = 9 * DAY_NS;
    });
    assert!(
        below_normal
            .validate()
            .unwrap_err()
            .contains("longer than the normal")
    );
    let equal = with_policy(|p| {
        p.normal_delay_ns = 5 * DAY_NS;
        p.sovereign_delay_ns = 5 * DAY_NS;
    });
    assert!(
        equal
            .validate()
            .unwrap_err()
            .contains("longer than the normal")
    );
    let long = with_policy(|p| p.sovereign_delay_ns = MAX_DELAY_NS + 1);
    assert!(long.validate().unwrap_err().contains("30 days"));
    let floors = with_policy(|p| {
        p.normal_delay_ns = MIN_NORMAL_DELAY_NS;
        p.sovereign_delay_ns = MIN_SOVEREIGN_DELAY_NS;
    });
    assert_eq!(floors.validate(), Ok(()));
}

#[test]
fn release_signers_are_optional_but_well_formed() {
    let none = with_policy(|p| {
        p.release_signers.clear();
        p.release_threshold = 0;
    });
    assert_eq!(none.validate(), Ok(()));
    assert!(none.signers().is_none());
    let dangling = with_policy(|p| {
        p.release_signers.clear();
        p.release_threshold = 1;
    });
    assert!(dangling.validate().is_err());
    let over = with_policy(|p| p.release_threshold = 4);
    assert!(over.validate().is_err());
    let zero = with_policy(|p| p.release_threshold = 0);
    assert!(zero.validate().is_err());
    // A recovery key that signs releases too would approve its own release.
    let shared = with_policy(|p| p.release_signers[1] = public(2));
    assert!(shared.validate().unwrap_err().contains("apart"));
}

#[test]
fn routes_have_their_own_threshold_and_delay() {
    let policy = policy();
    let signed = install(Some(signed_release(&wasm(), &[21, 22])));
    assert_eq!(signed.route(), Route::Release);
    assert_eq!(
        policy.requirement(signed.route()),
        Requirement {
            threshold: 1,
            delay_ns: 48 * HOUR_NS
        }
    );
    let repair = install(None);
    assert_eq!(repair.route(), Route::Sovereign);
    assert_eq!(
        policy.requirement(repair.route()),
        Requirement {
            threshold: 2,
            delay_ns: 7 * DAY_NS
        }
    );
}

#[test]
fn a_signed_release_must_name_the_operations_module_and_meet_the_threshold() {
    let policy = policy();
    let signed = install(Some(signed_release(&wasm(), &[21, 22])));
    assert_eq!(
        check_proposal(&signed, &context(&policy)),
        Ok(Route::Release)
    );

    let one = install(Some(signed_release(&wasm(), &[21])));
    let reason = check_proposal(&one, &context(&policy)).unwrap_err();
    assert!(reason.contains("1 of the 2 signatures"), "{reason}");

    // Signed by keys this controller does not trust.
    let strangers = install(Some(signed_release(&wasm(), &[1, 2])));
    assert!(check_proposal(&strangers, &context(&policy)).is_err());

    // A release of another module, attached to this one.
    let mut other = wasm();
    other[0] ^= 1;
    let substituted = install(Some(signed_release(&other, &[21, 22])));
    let reason = check_proposal(&substituted, &context(&policy)).unwrap_err();
    assert!(reason.contains("another module"), "{reason}");

    // Only a plain upgrade takes the release route.
    let mut skipping = signed.clone();
    if let OperationKind::InstallCode(install) = &mut skipping.kind {
        install.mode = InstallMode::Upgrade {
            skip_pre_upgrade: true,
        };
    }
    assert!(check_proposal(&skipping, &context(&policy)).is_err());
    let mut reinstall = signed.clone();
    if let OperationKind::InstallCode(install) = &mut reinstall.kind {
        install.mode = InstallMode::Reinstall;
    }
    assert!(check_proposal(&reinstall, &context(&policy)).is_err());

    let unsigned_channel = with_policy(|p| {
        p.release_signers.clear();
        p.release_threshold = 0;
    });
    let reason = check_proposal(&signed, &context(&unsigned_channel)).unwrap_err();
    assert!(reason.contains("sovereign route"), "{reason}");
    assert_eq!(
        check_proposal(&install(None), &context(&unsigned_channel)),
        Ok(Route::Sovereign)
    );
}

#[test]
fn a_proposal_is_bound_to_this_controller_wallet_version_nonce_and_window() {
    let policy = policy();
    let ok = install(None);
    assert!(check_proposal(&ok, &context(&policy)).is_ok());

    type Change = Box<dyn Fn(&mut Operation)>;
    let cases: Vec<(&str, Change)> = vec![
        ("for controller", Box::new(|op| op.controller = wallet())),
        ("for wallet", Box::new(|op| op.wallet = controller())),
        ("policy version", Box::new(|op| op.policy_version = 2)),
        ("nonce", Box::new(|op| op.nonce = 1)),
        ("after its delay", Box::new(|op| op.expires_at_ns = T0)),
        // A repair waits 7 days, so it may not expire before then.
        (
            "after its delay of 168 hours",
            Box::new(|op| op.expires_at_ns = T0 + 7 * DAY_NS),
        ),
        (
            "60 days",
            Box::new(|op| op.expires_at_ns = T0 + MAX_LIFETIME_NS + 1),
        ),
    ];
    for (expected, change) in cases {
        let mut operation = ok.clone();
        change(&mut operation);
        let reason = check_proposal(&operation, &context(&policy)).unwrap_err();
        assert!(reason.contains(expected), "{expected}: {reason}");
    }
    let mut soonest = ok.clone();
    soonest.expires_at_ns = T0 + 7 * DAY_NS + 1;
    assert!(check_proposal(&soonest, &context(&policy)).is_ok());
    // A signed release waits 48 hours.
    let mut release = install(Some(signed_release(&wasm(), &[21, 22])));
    release.expires_at_ns = T0 + 48 * HOUR_NS;
    assert!(
        check_proposal(&release, &context(&policy))
            .unwrap_err()
            .contains("48 hours")
    );
    release.expires_at_ns += 1;
    assert_eq!(
        check_proposal(&release, &context(&policy)),
        Ok(Route::Release)
    );
}

#[test]
fn an_install_has_well_formed_hashes_and_a_bounded_argument() {
    let policy = policy();
    let change = |f: &dyn Fn(&mut InstallCode)| {
        let mut operation = install(None);
        if let OperationKind::InstallCode(install) = &mut operation.kind {
            f(install);
        }
        check_proposal(&operation, &context(&policy))
    };
    assert!(change(&|i| i.wasm_sha256 = ByteBuf::from(vec![0; 31])).is_err());
    assert!(change(&|i| i.chunk_hashes.clear()).is_err());
    assert!(change(&|i| i.chunk_hashes = vec![ByteBuf::from(vec![0; 32]); 101]).is_err());
    assert!(change(&|i| i.chunk_hashes[1] = ByteBuf::from(vec![0; 33])).is_err());
    assert!(change(&|i| i.arg = ByteBuf::from(vec![0; MAX_ARG_LEN + 1])).is_err());
    assert!(change(&|i| i.arg = ByteBuf::from(vec![0; MAX_ARG_LEN])).is_ok());
}

#[test]
fn every_new_key_needs_a_proof_for_this_controller_and_wallet() {
    let current = policy();
    let mut next = policy();
    next.authorities[2] = Authority {
        public_key: public(4),
        label: "new".to_string(),
    };
    let set = |proofs: Vec<Proof>| Operation {
        kind: OperationKind::SetPolicy {
            policy: next.clone(),
            proofs,
        },
        ..install(None)
    };
    let reason = check_proposal(&set(vec![]), &context(&current)).unwrap_err();
    assert!(reason.contains("needs a proof"), "{reason}");

    assert_eq!(
        check_proposal(
            &set(vec![proof(4, controller(), wallet())]),
            &context(&current)
        ),
        Ok(Route::Sovereign)
    );

    // A proof made for another controller, or another wallet, is no proof
    // here.
    let reason =
        check_proposal(&set(vec![proof(4, wallet(), wallet())]), &context(&current)).unwrap_err();
    assert!(reason.contains("does not verify"), "{reason}");
    assert!(
        check_proposal(
            &set(vec![proof(4, controller(), controller())]),
            &context(&current)
        )
        .is_err()
    );

    // No proofs for keys outside the policy, and one per key.
    assert!(
        check_proposal(
            &set(vec![
                proof(4, controller(), wallet()),
                proof(9, controller(), wallet())
            ]),
            &context(&current)
        )
        .is_err()
    );
    assert!(
        check_proposal(
            &set(vec![
                proof(4, controller(), wallet()),
                proof(4, controller(), wallet())
            ]),
            &context(&current)
        )
        .is_err()
    );

    // At install every key needs one.
    assert!(check_proofs(&current, &[], &[], controller(), wallet()).is_err());
    let all: Vec<Proof> = (1..=3)
        .map(|seed| proof(seed, controller(), wallet()))
        .collect();
    assert_eq!(
        check_proofs(&current, &all, &[], controller(), wallet()),
        Ok(())
    );
}

#[test]
fn a_controller_is_not_handed_to_itself_or_the_wallet() {
    let policy = policy();
    let replace = |to: Principal| Operation {
        kind: OperationKind::ReplaceController {
            controller: to,
            module_hash: ByteBuf::from(vec![1; 32]),
            policy_sha256: ByteBuf::from(vec![2; 32]),
        },
        ..install(None)
    };
    let other = Principal::from_slice(&[0, 0, 0, 0, 0, 0, 0, 3, 1, 1]);
    assert_eq!(
        check_proposal(&replace(other), &context(&policy)),
        Ok(Route::Sovereign)
    );
    assert!(check_proposal(&replace(controller()), &context(&policy)).is_err());
    assert!(check_proposal(&replace(wallet()), &context(&policy)).is_err());
    assert!(check_proposal(&replace(Principal::anonymous()), &context(&policy)).is_err());
    assert!(
        check_proposal(
            &replace(Principal::management_canister()),
            &context(&policy)
        )
        .is_err()
    );
}

#[test]
fn a_key_cannot_cancel_its_own_removal() {
    let mut next = policy();
    next.authorities[2] = Authority {
        public_key: public(4),
        label: "key 4".to_string(),
    };
    let removal = Operation {
        kind: OperationKind::SetPolicy {
            policy: next,
            proofs: vec![],
        },
        ..install(None)
    };
    assert!(removal.removes(&public(3)));
    assert!(!removal.removes(&public(1)));
    assert!(!install(None).removes(&public(3)));
}

#[test]
fn signatures_verify_strictly_and_per_message() {
    let digest = install(None).digest();
    let signature = key(1).sign(&approve_message(&digest)).to_bytes();
    assert!(verify(&public(1), &approve_message(&digest), &signature));
    // An approval is not a cancellation, nor for another key or operation.
    assert!(!verify(&public(1), &cancel_message(&digest), &signature));
    assert!(!verify(&public(2), &approve_message(&digest), &signature));
    let other = install(Some(signed_release(&wasm(), &[21, 22]))).digest();
    assert!(!verify(&public(1), &approve_message(&other), &signature));
    assert!(!verify(
        &public(1),
        &approve_message(&digest),
        &signature[..63]
    ));
    // A signature whose scalar is not reduced is refused, so it cannot be
    // reshaped into a second valid one.
    let mut malleable = signature;
    malleable[63] |= 0xf0;
    assert!(!verify(&public(1), &approve_message(&digest), &malleable));
}

#[test]
fn messages_never_read_as_an_ic_request() {
    let digest = [7; 32];
    for message in [
        approve_message(&digest),
        cancel_message(&digest),
        enroll_message(controller(), wallet(), &[1; 32]),
        encode::policy_preimage(&policy()),
        encode::operation_preimage(&install(None)),
    ] {
        assert!(message.starts_with(b"b3wallet-controller/"));
        assert!(!message.starts_with(b"\x0aic-request"));
        assert!(!message.starts_with(b"\x1aic-request-auth-delegation"));
    }
}

#[test]
fn a_keys_principal_is_its_self_authenticating_principal() {
    let principal = principal_of(&public(1)).unwrap();
    assert_eq!(principal.as_slice().len(), 29);
    assert_eq!(principal.as_slice()[28], 0x02);
    assert_ne!(principal_of(&public(2)), Some(principal));
    assert_eq!(principal_of(&[0; 31]), None);
    let policy = policy();
    assert_eq!(
        policy.authority_of(&principal).map(|a| a.label.as_str()),
        Some("key 1")
    );
    assert_eq!(
        policy.authority_of(&principal_of(&public(4)).unwrap()),
        None
    );
}

/// Pinned digests, for a client written in another language to check its
/// encoding against (`docs/recovery.md` lists the same).
#[test]
fn test_vectors() {
    let to_hex = |bytes: [u8; 32]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let install = install(None);
    let mut next = policy();
    next.sovereign_delay_ns = 14 * DAY_NS;
    let set_policy = Operation {
        nonce: 1,
        kind: OperationKind::SetPolicy {
            policy: next,
            proofs: vec![],
        },
        ..install.clone()
    };
    let replace = Operation {
        nonce: 2,
        kind: OperationKind::ReplaceController {
            controller: Principal::from_slice(&[0, 0, 0, 0, 0, 0, 0, 3, 1, 1]),
            module_hash: ByteBuf::from(vec![1; 32]),
            policy_sha256: ByteBuf::from(policy().sha256().to_vec()),
        },
        ..install.clone()
    };
    let mut release = install.clone();
    if let OperationKind::InstallCode(code) = &mut release.kind {
        code.release = Some(signed_release(&wasm(), &[21, 22]));
    }
    let vectors = [
        ("policy", to_hex(policy().sha256())),
        ("install", to_hex(install.digest())),
        ("release", to_hex(release.digest())),
        ("set_policy", to_hex(set_policy.digest())),
        ("replace", to_hex(replace.digest())),
    ];
    println!("{vectors:#?}");
    assert_eq!(
        vectors,
        [
            ("policy", PINNED_POLICY.to_string()),
            ("install", PINNED_INSTALL.to_string()),
            ("release", PINNED_RELEASE.to_string()),
            ("set_policy", PINNED_SET_POLICY.to_string()),
            ("replace", PINNED_REPLACE.to_string()),
        ]
    );
}

const PINNED_POLICY: &str = "f386a4d210393f76bdf24ea9f4303327f6896ffaf3bd1477b4b0902cf62d1fe1";
const PINNED_INSTALL: &str = "3b7cc80f258157d2c400aa54f3715a326f4c9d2f5ee61344e0213e29f3c3250c";
const PINNED_RELEASE: &str = "a1243672325ea5f24965f7d9301d48c5cc3ef084613d2a4e0362126aea40b584";
const PINNED_SET_POLICY: &str = "905ed5a04f6fc07c386f7c422e6d7ca7ffa73d0c28e27bad6208880dd6799627";
const PINNED_REPLACE: &str = "631db29dc67bd3a3c4259f56b6f538c25a5919346bee4fd21accb4437a8aaa96";

/// `install(None)` and a release of its module, made once: hashing the
/// module for every case would make the property tests slow.
fn cached() -> &'static (Operation, ReleaseManifest) {
    static CACHED: std::sync::OnceLock<(Operation, ReleaseManifest)> = std::sync::OnceLock::new();
    CACHED.get_or_init(|| (install(None), signed_release(&wasm(), &[21, 22])))
}

fn principal_strategy() -> impl Strategy<Value = Principal> {
    prop::collection::vec(any::<u8>(), 0..=29).prop_map(|bytes| Principal::from_slice(&bytes))
}

proptest! {
    /// Changing any field of an operation changes its digest.
    #[test]
    fn every_field_is_in_the_digest(
        controller in principal_strategy(),
        wallet in principal_strategy(),
        policy_version in any::<u64>(),
        nonce in any::<u64>(),
        expires_at_ns in any::<u64>(),
        skip in any::<bool>(),
        arg in prop::collection::vec(any::<u8>(), 0..64),
        field in 0usize..16,
    ) {
        let (base, release) = cached();
        let mut base = base.clone();
        base.controller = controller;
        base.wallet = wallet;
        base.policy_version = policy_version;
        base.nonce = nonce;
        base.expires_at_ns = expires_at_ns;
        if let OperationKind::InstallCode(install) = &mut base.kind {
            install.mode = InstallMode::Upgrade { skip_pre_upgrade: skip };
            install.arg = ByteBuf::from(arg);
        }
        let mut changed = base.clone();
        match field {
            0 => changed.controller = Principal::from_slice(&[0xff; 29]),
            1 => changed.wallet = Principal::from_slice(&[0xfe; 29]),
            2 => changed.policy_version = policy_version.wrapping_add(1),
            3 => changed.nonce = nonce.wrapping_add(1),
            4 => changed.expires_at_ns = expires_at_ns.wrapping_add(1),
            _ => if let OperationKind::InstallCode(install) = &mut changed.kind {
                match field {
                    5 => install.mode = InstallMode::Upgrade { skip_pre_upgrade: !skip },
                    6 => install.mode = InstallMode::Reinstall,
                    7 => install.wasm_sha256[0] ^= 1,
                    8 => install.chunk_hashes[1][31] ^= 1,
                    9 => install.arg.push(0),
                    _ => install.release = Some(release.clone()),
                }
            },
        }
        // The fields of a release, each changed on an operation that has one.
        if field >= 11 {
            let OperationKind::InstallCode(install) = &mut base.kind else { unreachable!() };
            install.release = Some(release.clone());
            changed = base.clone();
            let OperationKind::InstallCode(install) = &mut changed.kind else { unreachable!() };
            let manifest = install.release.as_mut().unwrap();
            match field {
                11 => manifest.version.push('1'),
                12 => manifest.wasm_size += 1,
                13 => manifest.tested_from[0][0] ^= 1,
                14 => manifest.notes_url.push('/'),
                _ => manifest.published_at_ns += 1,
            }
        }
        prop_assume!(changed != base);
        prop_assert_ne!(changed.digest(), base.digest());
    }

    /// Changing any field of a policy changes its hash.
    #[test]
    fn every_policy_field_is_in_its_hash(field in 0usize..8) {
        let base = policy();
        let mut changed = base.clone();
        match field {
            0 => changed.authorities[0].public_key = public(9),
            1 => changed.authorities[1].label.push('!'),
            2 => changed.normal_threshold = 2,
            3 => changed.sovereign_threshold = 1,
            4 => changed.normal_delay_ns += 1,
            5 => changed.sovereign_delay_ns += 1,
            6 => { changed.release_signers.pop(); }
            _ => changed.release_threshold = 3,
        }
        prop_assert_ne!(changed.sha256(), base.sha256());
    }
}
