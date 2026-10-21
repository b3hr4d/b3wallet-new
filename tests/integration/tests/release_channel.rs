//! The real release channel Wasm in PocketIC. Build it first:
//! `cargo build -p release_channel --target wasm32-unknown-unknown --release`.
//! PocketIC uses the server binary named by `POCKET_IC_BIN`.
//!
//! Chunks and manifests reach the channel as `release-tool` writes them:
//! chunks as the bytes of its `chunk-N.bin` files, manifests as its Candid
//! text, typed with `candid/release_channel.did` the way icp-cli types it.

use candid::types::{Type, TypeInner};
use candid::{CandidType, Principal, TypeEnv, decode_one, encode_args, encode_one};
use candid_parser::utils::CandidSource;
use ed25519_dalek::SigningKey;
use pocket_ic::{ErrorCode, PocketIc, RejectCode};
use release::channel::{
    ChannelArg, ChannelConfig, ChannelError, ChunkRequest, PublishChunk, ReleaseSummary,
};
use release::{CHUNK_SIZE, MAX_CHUNKS, ReleaseManifest, sha256};
use release_channel::NOT_A_PUBLISHER;
use release_tool::{chunk_args, to_text};
use serde::de::DeserializeOwned;
use serde_bytes::ByteBuf;
use std::path::Path;

fn wasm() -> Vec<u8> {
    let path = std::env::var("RELEASE_CHANNEL_WASM").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/wasm32-unknown-unknown/release/release_channel.wasm"
        )
        .to_string()
    });
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!("cannot read {path} ({e}); build it with `cargo build -p release_channel --target wasm32-unknown-unknown --release`")
    })
}

fn signer(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn public_key(seed: u8) -> ByteBuf {
    ByteBuf::from(signer(seed).verifying_key().to_bytes())
}

fn publisher() -> Principal {
    Principal::self_authenticating([7; 32])
}

fn stranger() -> Principal {
    Principal::self_authenticating([8; 32])
}

/// Two of the keys 1, 2 and 3.
fn config() -> ChannelConfig {
    ChannelConfig {
        publishers: vec![publisher()],
        release_signers: vec![public_key(1), public_key(2), public_key(3)],
        release_threshold: 2,
    }
}

fn try_install(pic: &PocketIc, id: Principal, arg: &ChannelArg) -> Result<(), String> {
    pic.reinstall_canister(id, wasm(), encode_one(arg).unwrap(), None)
        .map_err(|reject| format!("{reject:?}"))
}

fn channel(pic: &PocketIc) -> Principal {
    let id = pic.create_canister();
    pic.add_cycles(id, 10_000_000_000_000);
    try_install(pic, id, &ChannelArg::Init(config())).unwrap();
    id
}

/// An upgrade in a later round, as in the wallet's tests.
fn upgrade(pic: &PocketIc, id: Principal, arg: &ChannelArg) -> Result<(), String> {
    pic.tick();
    pic.upgrade_canister(id, wasm(), encode_one(arg).unwrap(), None)
        .map_err(|reject| format!("{reject:?}"))
}

/// A stand-in for a wallet release: two full chunks and half of a third.
fn release_wasm(seed: u8) -> Vec<u8> {
    (0..2 * CHUNK_SIZE + CHUNK_SIZE / 2)
        .map(|i| (i as u8).wrapping_mul(13).wrapping_add(seed))
        .collect()
}

fn manifest(version: &str, wasm: &[u8]) -> ReleaseManifest {
    ReleaseManifest::new(
        version.to_string(),
        wasm,
        vec![[0xab; 32]],
        format!("https://example.org/releases/{version}"),
        1_700_000_000_000_000_000,
    )
}

fn signed_by(mut manifest: ReleaseManifest, seeds: &[u8]) -> ReleaseManifest {
    for &seed in seeds {
        manifest.sign(&signer(seed));
    }
    manifest
}

fn query<T: CandidType, R: DeserializeOwned + CandidType>(
    pic: &PocketIc,
    id: Principal,
    method: &str,
    arg: T,
) -> R {
    let reply = pic
        .query_call(id, Principal::anonymous(), method, encode_one(arg).unwrap())
        .unwrap_or_else(|reject| panic!("{method}: {reject:?}"));
    decode_one(&reply).unwrap()
}

fn latest(pic: &PocketIc, id: Principal) -> Option<ReleaseManifest> {
    query(pic, id, "latest", ())
}

fn get_manifest(pic: &PocketIc, id: Principal, version: &str) -> Option<ReleaseManifest> {
    query(pic, id, "manifest", version)
}

fn releases(pic: &PocketIc, id: Principal) -> Vec<ReleaseSummary> {
    query(pic, id, "releases", ())
}

fn chunk(pic: &PocketIc, id: Principal, version: &str, index: u32) -> Option<ByteBuf> {
    let request = ChunkRequest {
        version: version.to_string(),
        index,
    };
    query(pic, id, "chunk", request)
}

fn publish_chunk_bytes(pic: &PocketIc, id: Principal, arg: Vec<u8>) -> Result<(), ChannelError> {
    let reply = pic
        .update_call(id, publisher(), "publish_chunk", arg)
        .unwrap_or_else(|reject| panic!("publish_chunk: {reject:?}"));
    decode_one(&reply).unwrap()
}

fn publish_chunk(
    pic: &PocketIc,
    id: Principal,
    version: &str,
    index: u32,
    data: &[u8],
) -> Result<(), ChannelError> {
    let arg = PublishChunk {
        version: version.to_string(),
        index,
        data: ByteBuf::from(data),
    };
    publish_chunk_bytes(pic, id, encode_one(arg).unwrap())
}

/// Stages every chunk of `wasm`, from `release-tool chunks`.
fn stage(pic: &PocketIc, id: Principal, version: &str, wasm: &[u8]) {
    for arg in chunk_args(version, wasm) {
        assert_eq!(publish_chunk_bytes(pic, id, arg), Ok(()));
    }
}

/// The argument types of a method of `candid/release_channel.did`.
fn method_args(method: &str) -> (TypeEnv, Vec<Type>) {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../candid/release_channel.did"
    );
    let (env, actor) = CandidSource::File(Path::new(path))
        .load()
        .expect("candid/release_channel.did parses");
    let actor = env.trace_type(&actor.expect("a service")).unwrap();
    let TypeInner::Class(_, service) = actor.as_ref() else {
        panic!("the service has no init arguments: {actor}");
    };
    let (_, func) = env
        .as_service(service)
        .unwrap()
        .iter()
        .find(|(name, _)| name == method)
        .unwrap_or_else(|| panic!("{method} is not in candid/release_channel.did"))
        .clone();
    let args = env.as_func(&func).unwrap().args.clone();
    (env, args)
}

/// The manifest as `release-tool` writes it, encoded the way icp-cli
/// encodes a `--args-file` with the interface file.
fn manifest_arg(manifest: &ReleaseManifest) -> Vec<u8> {
    let (env, types) = method_args("publish_manifest");
    candid_parser::parse_idl_args(&to_text(manifest))
        .unwrap()
        .to_bytes_with_types(&env, &types)
        .unwrap()
}

fn publish_manifest(
    pic: &PocketIc,
    id: Principal,
    manifest: &ReleaseManifest,
) -> Result<(), ChannelError> {
    let reply = pic
        .update_call(id, publisher(), "publish_manifest", manifest_arg(manifest))
        .unwrap_or_else(|reject| panic!("publish_manifest: {reject:?}"));
    decode_one(&reply).unwrap()
}

/// What a wallet gets back: every chunk of `version`, put together.
fn served_wasm(pic: &PocketIc, id: Principal, manifest: &ReleaseManifest) -> Vec<u8> {
    let mut wasm = Vec::new();
    for index in 0..manifest.chunk_hashes.len() as u32 {
        let data = chunk(pic, id, &manifest.version, index).expect("a published chunk");
        wasm.extend_from_slice(&data);
    }
    assert_eq!(
        chunk(
            pic,
            id,
            &manifest.version,
            manifest.chunk_hashes.len() as u32
        ),
        None
    );
    wasm
}

fn caller_wasm() -> Vec<u8> {
    let path = std::env::var("CALLER_WASM").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/wasm32-unknown-unknown/release/caller.wasm"
        )
        .to_string()
    });
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!("cannot read {path} ({e}); build it with `cargo build -p caller --target wasm32-unknown-unknown --release`")
    })
}

/// A canister that forwards calls, from `tests/caller`.
fn caller(pic: &PocketIc) -> Principal {
    let id = pic.create_canister();
    pic.add_cycles(id, 2_000_000_000_000);
    pic.install_canister(id, caller_wasm(), encode_one(()).unwrap(), None);
    id
}

/// Calls `method` of `canister` from the `caller` canister.
fn forward(
    pic: &PocketIc,
    caller: Principal,
    canister: Principal,
    method: &str,
    arg: Vec<u8>,
) -> Result<Vec<u8>, (u32, String)> {
    let reply = pic
        .update_call(
            caller,
            Principal::anonymous(),
            "forward",
            encode_args((canister, method, ByteBuf::from(arg))).unwrap(),
        )
        .expect("forward answers");
    decode_one::<Result<ByteBuf, (u32, String)>>(&reply)
        .unwrap()
        .map(ByteBuf::into_vec)
}

fn not_live(pic: &PocketIc, id: Principal, version: &str) {
    assert_eq!(get_manifest(pic, id, version), None);
    assert_eq!(chunk(pic, id, version, 0), None);
    assert!(releases(pic, id).iter().all(|r| r.version != version));
}

#[test]
fn two_of_three_signers_publish_a_release() {
    let pic = PocketIc::new();
    let id = channel(&pic);
    assert_eq!(latest(&pic, id), None);
    assert_eq!(releases(&pic, id), vec![]);

    let wasm = release_wasm(1);
    let manifest = signed_by(manifest("1.0.0", &wasm), &[1, 3]);
    stage(&pic, id, "1.0.0", &wasm);
    // Staged chunks are not served.
    not_live(&pic, id, "1.0.0");

    assert_eq!(publish_manifest(&pic, id, &manifest), Ok(()));
    assert_eq!(latest(&pic, id), Some(manifest.clone()));
    assert_eq!(get_manifest(&pic, id, "1.0.0"), Some(manifest.clone()));
    let served = served_wasm(&pic, id, &manifest);
    assert_eq!(served, wasm);
    assert_eq!(sha256(&served), manifest.wasm_sha256.as_slice());

    let next_wasm = release_wasm(2);
    let next = signed_by(self::manifest("1.0.1", &next_wasm), &[2, 3]);
    stage(&pic, id, "1.0.1", &next_wasm);
    assert_eq!(publish_manifest(&pic, id, &next), Ok(()));
    assert_eq!(latest(&pic, id), Some(next.clone()));
    assert_eq!(served_wasm(&pic, id, &manifest), wasm);
    assert_eq!(served_wasm(&pic, id, &next), next_wasm);
    assert_eq!(
        releases(&pic, id),
        [&manifest, &next]
            .map(|m| ReleaseSummary {
                version: m.version.clone(),
                wasm_sha256: m.wasm_sha256.clone(),
                published_at_ns: m.published_at_ns,
                notes_url: m.notes_url.clone(),
            })
            .to_vec()
    );
}

#[test]
fn a_manifest_with_too_few_signatures_is_refused() {
    let pic = PocketIc::new();
    let id = channel(&pic);
    let wasm = release_wasm(1);
    stage(&pic, id, "1.0.0", &wasm);
    let unsigned = manifest("1.0.0", &wasm);

    let tampered = {
        let mut m = signed_by(unsigned.clone(), &[1, 2]);
        m.signatures[1].signature[0] ^= 1;
        m
    };
    let over_another_manifest = {
        let mut other = unsigned.clone();
        other.notes_url = "https://example.org/other".to_string();
        let other = signed_by(other, &[2]);
        let mut m = signed_by(unsigned.clone(), &[1]);
        m.signatures.push(other.signatures[0].clone());
        m
    };
    let twice = {
        let mut m = signed_by(unsigned.clone(), &[1]);
        m.signatures.push(m.signatures[0].clone());
        m
    };
    let cases = [
        ("no signature", unsigned.clone(), 0),
        ("one signer", signed_by(unsigned.clone(), &[1]), 1),
        ("one signer twice", twice, 1),
        (
            "a key the channel does not pin",
            signed_by(unsigned.clone(), &[1, 9]),
            1,
        ),
        ("a changed signature", tampered, 1),
        (
            "a signature over another manifest",
            over_another_manifest,
            1,
        ),
    ];
    for (case, manifest, have) in cases {
        assert_eq!(
            publish_manifest(&pic, id, &manifest),
            Err(ChannelError::NotEnoughSignatures { have, need: 2 }),
            "{case}"
        );
        not_live(&pic, id, "1.0.0");
    }

    let manifest = signed_by(unsigned, &[1, 2]);
    assert_eq!(publish_manifest(&pic, id, &manifest), Ok(()));
    assert_eq!(served_wasm(&pic, id, &manifest), wasm);
}

#[test]
fn a_wrong_or_missing_chunk_is_refused() {
    let pic = PocketIc::new();
    let id = channel(&pic);
    let wasm = release_wasm(1);
    let manifest = signed_by(manifest("1.0.0", &wasm), &[1, 2]);

    assert_eq!(
        publish_manifest(&pic, id, &manifest),
        Err(ChannelError::MissingChunk { index: 0 })
    );
    stage(&pic, id, "1.0.0", &wasm);
    let mut wrong = wasm[CHUNK_SIZE..2 * CHUNK_SIZE].to_vec();
    wrong[12345] ^= 1;
    assert_eq!(publish_chunk(&pic, id, "1.0.0", 1, &wrong), Ok(()));
    assert_eq!(
        publish_manifest(&pic, id, &manifest),
        Err(ChannelError::HashMismatch { index: 1 })
    );
    not_live(&pic, id, "1.0.0");

    // A chunk staged again replaces the wrong one.
    let right = &wasm[CHUNK_SIZE..2 * CHUNK_SIZE];
    assert_eq!(publish_chunk(&pic, id, "1.0.0", 1, right), Ok(()));
    assert_eq!(publish_manifest(&pic, id, &manifest), Ok(()));
    assert_eq!(served_wasm(&pic, id, &manifest), wasm);
}

#[test]
fn a_release_missing_its_last_chunk_is_refused() {
    let pic = PocketIc::new();
    let id = channel(&pic);
    let wasm = release_wasm(1);
    let manifest = signed_by(manifest("1.0.0", &wasm), &[1, 2]);
    stage(&pic, id, "1.0.0", &wasm[..2 * CHUNK_SIZE]);
    assert_eq!(
        publish_manifest(&pic, id, &manifest),
        Err(ChannelError::MissingChunk { index: 2 })
    );
    not_live(&pic, id, "1.0.0");
}

#[test]
fn chunks_that_are_not_the_named_wasm_are_refused() {
    let pic = PocketIc::new();
    let id = channel(&pic);
    let wasm = release_wasm(1);
    stage(&pic, id, "1.0.0", &wasm);

    let mut other_wasm = manifest("1.0.0", &wasm);
    other_wasm.wasm_sha256 = ByteBuf::from(sha256(&release_wasm(2)));
    let other_wasm = signed_by(other_wasm, &[1, 2]);
    assert_eq!(
        publish_manifest(&pic, id, &other_wasm),
        Err(ChannelError::WasmMismatch)
    );

    // Every chunk matches its hash, but the Wasm is cut at another place.
    let cut = [
        &wasm[..CHUNK_SIZE - 100],
        &wasm[CHUNK_SIZE - 100..2 * CHUNK_SIZE - 100],
        &wasm[2 * CHUNK_SIZE - 100..],
    ];
    for (index, data) in cut.iter().enumerate() {
        assert_eq!(publish_chunk(&pic, id, "1.0.0", index as u32, data), Ok(()));
    }
    let mut miscut = manifest("1.0.0", &wasm);
    miscut.chunk_hashes = cut.iter().map(|c| ByteBuf::from(sha256(c))).collect();
    let miscut = signed_by(miscut, &[1, 2]);
    assert_eq!(
        publish_manifest(&pic, id, &miscut),
        Err(ChannelError::WasmMismatch)
    );
    not_live(&pic, id, "1.0.0");
}

#[test]
fn a_published_release_cannot_change() {
    let pic = PocketIc::new();
    let id = channel(&pic);
    let wasm = release_wasm(1);
    let manifest = signed_by(manifest("1.0.0", &wasm), &[1, 2]);
    stage(&pic, id, "1.0.0", &wasm);
    // A chunk past the last one, dropped when the release goes live.
    assert_eq!(publish_chunk(&pic, id, "1.0.0", 3, b"extra"), Ok(()));
    assert_eq!(publish_manifest(&pic, id, &manifest), Ok(()));
    assert_eq!(chunk(&pic, id, "1.0.0", 3), None);

    let other_wasm = release_wasm(2);
    let other = signed_by(self::manifest("1.0.0", &other_wasm), &[1, 2]);
    assert_eq!(
        publish_manifest(&pic, id, &other),
        Err(ChannelError::AlreadyPublished("1.0.0".to_string()))
    );
    assert_eq!(
        publish_manifest(&pic, id, &manifest),
        Err(ChannelError::AlreadyPublished("1.0.0".to_string()))
    );
    assert_eq!(
        publish_chunk(&pic, id, "1.0.0", 0, &other_wasm[..CHUNK_SIZE]),
        Err(ChannelError::AlreadyPublished("1.0.0".to_string()))
    );
    assert_eq!(get_manifest(&pic, id, "1.0.0"), Some(manifest.clone()));
    assert_eq!(served_wasm(&pic, id, &manifest), wasm);
}

#[test]
fn chunks_and_manifests_out_of_form_are_invalid() {
    let pic = PocketIc::new();
    let id = channel(&pic);
    let invalid = |result: Result<(), ChannelError>, names: &str| match result {
        Err(ChannelError::Invalid(reason)) => assert!(reason.contains(names), "{reason}"),
        other => panic!("{other:?}, not Invalid naming {names}"),
    };
    invalid(
        publish_chunk(&pic, id, "1.0.0", 0, &vec![0; CHUNK_SIZE + 1]),
        "1048576 bytes",
    );
    invalid(publish_chunk(&pic, id, "1.0.0", 0, &[]), "1048576 bytes");
    invalid(
        publish_chunk(&pic, id, "1.0.0", MAX_CHUNKS as u32, b"x"),
        "100 chunks",
    );
    invalid(publish_chunk(&pic, id, "1/0", 0, b"x"), "version");
    invalid(publish_chunk(&pic, id, "", 0, b"x"), "version");
    assert_eq!(
        publish_chunk(&pic, id, "1.0.0", MAX_CHUNKS as u32 - 1, b"x"),
        Ok(())
    );

    // The form is checked before the signatures.
    let wasm = release_wasm(1);
    let mut http = manifest("1.0.0", &wasm);
    http.notes_url = "http://example.org".to_string();
    invalid(publish_manifest(&pic, id, &http), "notes_url");
    let mut too_many = manifest("1.0.0", &wasm);
    too_many.chunk_hashes.push(ByteBuf::from(vec![0; 32]));
    invalid(
        publish_manifest(&pic, id, &signed_by(too_many, &[1, 2])),
        "chunks",
    );
}

#[test]
fn strangers_cannot_publish() {
    let pic = PocketIc::new();
    let id = channel(&pic);
    let wasm = release_wasm(1);
    let manifest = signed_by(manifest("1.0.0", &wasm), &[1, 2]);
    let calls = [
        ("publish_chunk", chunk_args("1.0.0", &wasm).remove(0)),
        ("publish_manifest", manifest_arg(&manifest)),
    ];
    let caller = caller(&pic);
    for (method, arg) in calls {
        // The message never reaches the canister, so it costs it nothing.
        for sender in [stranger(), Principal::anonymous()] {
            let reject = pic
                .update_call(id, sender, method, arg.clone())
                .expect_err(method);
            assert_eq!(
                reject.error_code,
                ErrorCode::CanisterRejectedMessage,
                "{method}"
            );
        }
        // A call from a canister skips that check and meets the guard.
        let (code, message) = forward(&pic, caller, id, method, arg).expect_err(method);
        assert_eq!(
            code,
            RejectCode::CanisterReject as u32,
            "{method}: {message}"
        );
        assert_eq!(message, NOT_A_PUBLISHER, "{method}");
    }
    not_live(&pic, id, "1.0.0");
}

#[test]
fn the_install_refuses_a_bad_configuration() {
    let pic = PocketIc::new();
    let id = pic.create_canister();
    pic.add_cycles(id, 2_000_000_000_000);
    let bad = [
        ChannelConfig {
            release_threshold: 4,
            ..config()
        },
        ChannelConfig {
            release_threshold: 0,
            ..config()
        },
        ChannelConfig {
            release_signers: vec![public_key(1), public_key(1)],
            ..config()
        },
        ChannelConfig {
            release_signers: vec![ByteBuf::from(vec![2; 32])],
            release_threshold: 1,
            ..config()
        },
        ChannelConfig {
            publishers: vec![Principal::anonymous()],
            ..config()
        },
    ];
    for config in bad {
        pic.tick();
        let err = try_install(&pic, id, &ChannelArg::Init(config.clone())).unwrap_err();
        assert!(err.contains("invalid ChannelConfig"), "{config:?}: {err}");
    }
    pic.tick();
    let err = try_install(&pic, id, &ChannelArg::Upgrade(None)).unwrap_err();
    assert!(err.contains("install needs ChannelArg::Init"), "{err}");
}

#[test]
fn upgrades_keep_releases_and_can_change_the_signers() {
    let pic = PocketIc::new();
    let id = channel(&pic);
    let wasm = release_wasm(1);
    let manifest = signed_by(manifest("1.0.0", &wasm), &[1, 2]);
    stage(&pic, id, "1.0.0", &wasm);
    assert_eq!(publish_manifest(&pic, id, &manifest), Ok(()));
    let staged = release_wasm(2);
    stage(&pic, id, "1.0.1", &staged);

    upgrade(&pic, id, &ChannelArg::Upgrade(None)).unwrap();
    assert_eq!(latest(&pic, id), Some(manifest.clone()));
    assert_eq!(served_wasm(&pic, id, &manifest), wasm);

    // An upgrade with install arguments, or a bad configuration, is refused
    // and the running version keeps its configuration.
    let err = upgrade(&pic, id, &ChannelArg::Init(config())).unwrap_err();
    assert!(
        err.contains("an upgrade needs ChannelArg::Upgrade"),
        "{err}"
    );
    let bad = ChannelConfig {
        release_threshold: 0,
        ..config()
    };
    let err = upgrade(&pic, id, &ChannelArg::Upgrade(Some(bad))).unwrap_err();
    assert!(err.contains("invalid ChannelConfig"), "{err}");

    // New signers: keys 4, 5 and 6. Staged chunks stay staged.
    let rotated = ChannelConfig {
        release_signers: vec![public_key(4), public_key(5), public_key(6)],
        ..config()
    };
    upgrade(&pic, id, &ChannelArg::Upgrade(Some(rotated))).unwrap();
    let next = self::manifest("1.0.1", &staged);
    assert_eq!(
        publish_manifest(&pic, id, &signed_by(next.clone(), &[1, 2])),
        Err(ChannelError::NotEnoughSignatures { have: 0, need: 2 })
    );
    let next = signed_by(next, &[4, 6]);
    assert_eq!(publish_manifest(&pic, id, &next), Ok(()));
    assert_eq!(served_wasm(&pic, id, &next), staged);
    assert_eq!(get_manifest(&pic, id, "1.0.0"), Some(manifest));
}

/// A release of `MAX_CHUNKS` full chunks, the most the IC's chunk store
/// takes, publishes in one call: `publish_manifest` reads and hashes all of
/// it. On 2026-10-21 that took about 16.4 billion of the 40 billion
/// instructions an update may use; PocketIC charges one cycle for each.
#[test]
#[ignore = "stages 100 MiB"]
fn the_largest_release_publishes_in_one_call() {
    let pic = PocketIc::new();
    let id = channel(&pic);
    let wasm: Vec<u8> = (0..MAX_CHUNKS * CHUNK_SIZE)
        .map(|i| (i as u8).wrapping_mul(29) ^ (i >> 20) as u8)
        .collect();
    let manifest = signed_by(manifest("1.0.0", &wasm), &[1, 2]);
    assert_eq!(manifest.chunk_hashes.len(), MAX_CHUNKS);
    stage(&pic, id, "1.0.0", &wasm);

    let before = pic.cycle_balance(id);
    assert_eq!(publish_manifest(&pic, id, &manifest), Ok(()));
    eprintln!(
        "publishing {MAX_CHUNKS} chunks cost {} cycles",
        before - pic.cycle_balance(id)
    );
    let last = chunk(&pic, id, "1.0.0", MAX_CHUNKS as u32 - 1).unwrap();
    assert_eq!(last.as_slice(), &wasm[(MAX_CHUNKS - 1) * CHUNK_SIZE..]);
}
