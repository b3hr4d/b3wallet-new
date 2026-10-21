use super::*;
use proptest::prelude::*;

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn keys() -> Vec<SigningKey> {
    vec![key(1), key(2), key(3)]
}

fn public_keys(keys: &[SigningKey]) -> Vec<[u8; 32]> {
    keys.iter().map(|k| k.verifying_key().to_bytes()).collect()
}

/// A Wasm of two full chunks and a short third one.
fn wasm() -> Vec<u8> {
    (0..2 * CHUNK_SIZE + 1000)
        .map(|i| (i % 251) as u8)
        .collect()
}

fn manifest(wasm: &[u8]) -> ReleaseManifest {
    ReleaseManifest::new(
        "1.2.3".to_string(),
        wasm,
        vec![[7; 32]],
        "https://example.org/releases/1.2.3".to_string(),
        1_700_000_000_000_000_000,
    )
}

fn two_of_three() -> Signers {
    Signers::new(&public_keys(&keys()), 2).unwrap()
}

#[test]
fn a_new_manifest_names_the_chunks_of_its_wasm() {
    let wasm = wasm();
    let manifest = manifest(&wasm);
    assert_eq!(manifest.wasm_size, wasm.len() as u64);
    assert_eq!(manifest.chunk_hashes.len(), 3);
    assert_eq!(manifest.chunk_size(0), CHUNK_SIZE);
    assert_eq!(manifest.chunk_size(2), 1000);
    assert_eq!(manifest.check_form(), Ok(()));
    assert_eq!(manifest.check_chunks(|i| chunks(&wasm).nth(i)), Ok(()));
}

#[test]
fn chunk_count_cuts_at_the_chunk_size() {
    assert_eq!(chunk_count(1), 1);
    assert_eq!(chunk_count(CHUNK_SIZE as u64), 1);
    assert_eq!(chunk_count(CHUNK_SIZE as u64 + 1), 2);
    assert_eq!(chunk_count((MAX_CHUNKS * CHUNK_SIZE) as u64), MAX_CHUNKS);
}

/// The signing hash is the one format a signer and a wallet must agree on
/// byte for byte, so its input and output are pinned here.
#[test]
fn the_signing_hash_is_pinned() {
    let manifest = ReleaseManifest {
        version: "1.0.0".to_string(),
        wasm_sha256: ByteBuf::from(vec![0xaa; 32]),
        wasm_size: 1000,
        chunk_hashes: vec![ByteBuf::from(vec![0xbb; 32])],
        tested_from: vec![ByteBuf::from(vec![0xcc; 32])],
        notes_url: "https://example.org/1.0.0".to_string(),
        published_at_ns: 1_700_000_000_000_000_000,
        signatures: vec![ReleaseSignature {
            signer: ByteBuf::from(vec![1; 32]),
            signature: ByteBuf::from(vec![2; 64]),
        }],
    };
    let unsigned = ReleaseManifest {
        signatures: Vec::new(),
        ..manifest.clone()
    };
    assert_eq!(hex(&encode_one(&unsigned).unwrap()), SIGNED_BYTES);
    assert_eq!(hex(&manifest.signing_hash()), SIGNING_HASH);
}

const SIGNED_BYTES: &str = "4449444c056c08b182e4840271f8eccdcf04018ef588f80501e2fee6df0602c0e98fc4077898cec7e70771fbec8bc60a03fced91e60f786d026d7b6d046c02f8c5aeab0102eae398a40b0201001968747470733a2f2f6578616d706c652e6f72672f312e302e300120cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc0120bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb20aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa00002a36fe9c971705312e302e3000e803000000000000";
const SIGNING_HASH: &str = "8bd9e06a4d32cbc8d736548b9265d36f647bc21199447bf59d251e4b659c03a6";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn the_signing_hash_covers_every_field_but_the_signatures() {
    let manifest = manifest(&wasm());
    let hash = manifest.signing_hash();
    let mut signed = manifest.clone();
    signed.sign(&key(1));
    assert_eq!(signed.signing_hash(), hash);

    let changes: Vec<fn(&mut ReleaseManifest)> = vec![
        |m| m.version.push('1'),
        |m| m.wasm_sha256[0] ^= 1,
        |m| m.wasm_size += 1,
        |m| m.chunk_hashes[1][0] ^= 1,
        |m| m.chunk_hashes.swap(0, 1),
        |m| m.tested_from.clear(),
        |m| m.notes_url.push('x'),
        |m| m.published_at_ns += 1,
    ];
    for change in changes {
        let mut changed = manifest.clone();
        change(&mut changed);
        assert_ne!(changed.signing_hash(), hash, "{changed:?}");
    }
}

#[test]
fn two_of_three_keys_accept_a_manifest() {
    let keys = keys();
    let signers = two_of_three();
    let mut manifest = manifest(&wasm());
    assert_eq!(
        signers.check(&manifest),
        Err(NotEnoughSignatures { have: 0, need: 2 })
    );
    manifest.sign(&keys[0]);
    assert_eq!(
        signers.check(&manifest),
        Err(NotEnoughSignatures { have: 1, need: 2 })
    );
    manifest.sign(&keys[2]);
    assert_eq!(signers.check(&manifest), Ok(()));
    manifest.sign(&keys[1]);
    assert_eq!(signers.count(&manifest), 3);
}

#[test]
fn signing_again_replaces_the_signature() {
    let mut manifest = manifest(&wasm());
    manifest.sign(&key(1));
    manifest.sign(&key(1));
    assert_eq!(manifest.signatures.len(), 1);
}

#[test]
fn a_key_counts_once() {
    let mut manifest = manifest(&wasm());
    manifest.sign(&key(1));
    let again = manifest.signatures[0].clone();
    manifest.signatures.push(again);
    assert_eq!(
        two_of_three().check(&manifest),
        Err(NotEnoughSignatures { have: 1, need: 2 })
    );
}

#[test]
fn another_key_counts_for_nothing() {
    let mut manifest = manifest(&wasm());
    manifest.sign(&key(1));
    manifest.sign(&key(9));
    assert_eq!(
        two_of_three().check(&manifest),
        Err(NotEnoughSignatures { have: 1, need: 2 })
    );
}

#[test]
fn a_signature_that_does_not_verify_counts_for_nothing() {
    let mut manifest = manifest(&wasm());
    manifest.sign(&key(1));
    manifest.sign(&key(2));
    manifest.signatures[1].signature[10] ^= 1;
    assert_eq!(two_of_three().count(&manifest), 1);

    // A signature over another manifest.
    let mut other = manifest.clone();
    other.notes_url = "https://example.org/other".to_string();
    other.sign(&key(2));
    manifest.signatures[1] = other.signatures[1].clone();
    assert_eq!(two_of_three().count(&manifest), 1);
}

/// Adding the group order to s gives a second encoding of the same
/// signature, which a strict check refuses.
#[test]
fn a_reshaped_signature_counts_for_nothing() {
    const ORDER: [u8; 32] = [
        0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde,
        0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
    ];
    let mut manifest = manifest(&wasm());
    manifest.sign(&key(1));
    manifest.sign(&key(2));
    let s = &mut manifest.signatures[1].signature[32..];
    let mut carry = 0u16;
    for (byte, order) in s.iter_mut().zip(ORDER) {
        let sum = u16::from(*byte) + u16::from(order) + carry;
        *byte = sum as u8;
        carry = sum >> 8;
    }
    assert_eq!(carry, 0);
    assert_eq!(two_of_three().count(&manifest), 1);
}

#[test]
fn signers_refuse_bad_keys_and_thresholds() {
    let good = public_keys(&keys());
    assert!(Signers::new(&good, 1).is_ok());
    assert!(Signers::new(&good, 3).is_ok());
    assert!(Signers::new(&good, 0).is_err());
    assert!(Signers::new(&good, 4).is_err());
    assert!(Signers::new::<[u8; 32]>(&[], 1).is_err());
    assert!(Signers::new(&[good[0], good[0]], 1).is_err());
    // Not a curve point, and the identity point, which is weak.
    assert!(Signers::new(&[[2; 32]], 1).is_err());
    let mut identity = [0; 32];
    identity[0] = 1;
    assert!(Signers::new(&[identity], 1).is_err());
    assert!(Signers::new(&[vec![1u8; 31]], 1).is_err());
    let many: Vec<[u8; 32]> = (1..=17)
        .map(|seed| key(seed).verifying_key().to_bytes())
        .collect();
    assert!(Signers::new(&many, 1).is_err());
    assert!(Signers::new(&many[..16], 1).is_ok());
}

#[test]
fn the_form_check_refuses_each_bad_field() {
    let good = manifest(&wasm());
    let changes: Vec<(Change<ReleaseManifest>, &str)> = vec![
        (|m| m.version.clear(), "version"),
        (|m| m.version = "1.0 beta".to_string(), "version"),
        (|m| m.version = "1/0".to_string(), "version"),
        (|m| m.version = "1".repeat(65), "version"),
        (
            |m| {
                m.wasm_sha256.pop();
            },
            "wasm_sha256",
        ),
        (|m| m.wasm_size = 0, "wasm_size"),
        (
            |m| m.wasm_size = (MAX_CHUNKS * CHUNK_SIZE) as u64 + 1,
            "wasm_size",
        ),
        (|m| m.wasm_size += CHUNK_SIZE as u64, "chunks"),
        (|m| drop(m.chunk_hashes.pop()), "chunks"),
        (|m| m.chunk_hashes[2].push(0), "chunk hash 2"),
        (
            |m| m.tested_from = vec![ByteBuf::from(vec![0; 32]); 65],
            "tested_from",
        ),
        (|m| m.tested_from[0].push(0), "tested_from hash 0"),
        (
            |m| m.notes_url = "http://example.org".to_string(),
            "notes_url",
        ),
        (
            |m| m.notes_url = "javascript:alert(1)".to_string(),
            "notes_url",
        ),
        (|m| m.notes_url.push(' '), "notes_url"),
        (|m| m.notes_url.push('\n'), "notes_url"),
        (|m| m.notes_url.push('é'), "notes_url"),
        (
            |m| m.notes_url = format!("https://{}", "a".repeat(505)),
            "notes_url",
        ),
        (
            |m| {
                for seed in 1..=17 {
                    m.sign(&key(seed));
                }
            },
            "signatures",
        ),
        (
            |m| {
                m.sign(&key(1));
                m.signatures[0].signature.pop();
            },
            "signature 0",
        ),
        (
            |m| {
                m.sign(&key(1));
                m.signatures[0].signer.push(0);
            },
            "signature 0",
        ),
    ];
    for (change, field) in changes {
        let mut changed = good.clone();
        change(&mut changed);
        let refusal = changed.check_form().expect_err(field);
        assert!(refusal.contains(field), "{refusal} names {field}");
    }

    let mut edge = good.clone();
    edge.version = "v1.0.0-rc.1+build_7".to_string();
    edge.notes_url = format!("https://{}", "a".repeat(504));
    edge.tested_from = vec![ByteBuf::from(vec![0; 32]); 64];
    for seed in 1..=16 {
        edge.sign(&key(seed));
    }
    assert_eq!(edge.check_form(), Ok(()));
}

#[test]
fn check_chunks_finds_a_missing_or_wrong_chunk() {
    let wasm = wasm();
    let manifest = manifest(&wasm);
    let mut chunks: Vec<Vec<u8>> = super::chunks(&wasm).map(<[u8]>::to_vec).collect();

    let missing = manifest.check_chunks(|i| (i != 1).then(|| chunks[i].clone()));
    assert_eq!(missing, Err(ChunkError::Missing { index: 1 }));

    chunks[2][999] ^= 1;
    let wrong = manifest.check_chunks(|i| chunks.get(i));
    assert_eq!(wrong, Err(ChunkError::HashMismatch { index: 2 }));
}

#[test]
fn check_chunks_refuses_chunks_that_are_not_the_wasm() {
    let wasm = wasm();
    let mut manifest = manifest(&wasm);
    manifest.wasm_sha256[0] ^= 1;
    assert_eq!(
        manifest.check_chunks(|i| chunks(&wasm).nth(i)),
        Err(ChunkError::WasmMismatch)
    );

    // The same Wasm cut in other places: each chunk matches its hash.
    let cut = [
        &wasm[..1000],
        &wasm[1000..CHUNK_SIZE + 1000],
        &wasm[CHUNK_SIZE + 1000..],
    ];
    let mut manifest = self::manifest(&wasm);
    manifest.chunk_hashes = cut.iter().map(|c| ByteBuf::from(sha256(c))).collect();
    assert_eq!(manifest.check_form(), Ok(()));
    assert_eq!(
        manifest.check_chunks(|i| cut.get(i)),
        Err(ChunkError::WasmMismatch)
    );
}

#[test]
fn check_wasm_refuses_bytes_past_the_wasm() {
    let wasm: Vec<u8> = (0..2 * CHUNK_SIZE).map(|i| (i % 251) as u8).collect();
    let manifest = manifest(&wasm);
    assert_eq!(manifest.check_wasm(&wasm), Ok(()));

    // The extra bytes make a third chunk, which the manifest never asks for.
    let longer = [wasm.as_slice(), b"extra"].concat();
    assert_eq!(manifest.check_chunks(|i| chunks(&longer).nth(i)), Ok(()));
    assert_eq!(manifest.check_wasm(&longer), Err(ChunkError::WasmMismatch));
    assert_eq!(
        manifest.check_wasm(&wasm[..CHUNK_SIZE]),
        Err(ChunkError::WasmMismatch)
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    /// Any Wasm passes against its own manifest, and a change to any one
    /// byte of it fails.
    #[test]
    fn a_changed_byte_fails_the_chunk_check(
        size in 1..3 * CHUNK_SIZE,
        seed: u8,
        at: prop::sample::Index,
    ) {
        let wasm: Vec<u8> = (0..size).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect();
        let manifest = manifest(&wasm);
        prop_assert_eq!(manifest.check_form(), Ok(()));
        prop_assert_eq!(manifest.check_chunks(|i| chunks(&wasm).nth(i)), Ok(()));

        let at = at.index(size);
        let mut changed = wasm.clone();
        changed[at] ^= 0x80;
        prop_assert_eq!(
            manifest.check_chunks(|i| chunks(&changed).nth(i)),
            Err(ChunkError::HashMismatch { index: (at / CHUNK_SIZE) as u32 })
        );
    }
}

/// A change that makes a valid value invalid.
type Change<T> = fn(&mut T);
