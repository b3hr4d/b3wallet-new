//! Runs the `release-tool` binary the way release signers do.

use candid::decode_one;
use ed25519_dalek::SigningKey;
use ed25519_dalek::pkcs8::EncodePrivateKey;
use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
use release::channel::PublishChunk;
use release::{CHUNK_SIZE, ReleaseManifest};
use release_tool::{from_text, hex};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_string()
    }

    /// Writes `count` keys, `key-1.pem` and up, and returns their public keys
    /// in hex.
    fn keys(&self, count: u8) -> Vec<String> {
        (1..=count)
            .map(|seed| {
                let key = SigningKey::from_bytes(&[seed; 32]);
                let pem = key.to_pkcs8_pem(LineEnding::LF).unwrap();
                std::fs::write(self.path(&format!("key-{seed}.pem")), pem.as_bytes()).unwrap();
                hex(key.verifying_key().as_bytes())
            })
            .collect()
    }

    /// A Wasm of two full chunks and a short third one.
    fn wasm(&self, name: &str, seed: u8) -> String {
        let wasm: Vec<u8> = (0..2 * CHUNK_SIZE + CHUNK_SIZE / 2)
            .map(|i| (i as u8).wrapping_add(seed))
            .collect();
        let path = self.path(name);
        std::fs::write(&path, wasm).unwrap();
        path
    }

    fn manifest(&self) -> ReleaseManifest {
        from_text(&std::fs::read_to_string(self.path("manifest.did")).unwrap()).unwrap()
    }
}

fn tool(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_release-tool"))
        .args(args)
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

/// Writes the unsigned manifest of `wasm.wasm` to `manifest.did`.
fn new_manifest(dir: &Dir) -> String {
    let wasm = dir.wasm("wasm.wasm", 0);
    let output = tool(&[
        "manifest",
        "--wasm",
        &wasm,
        "--version",
        "1.0.0",
        "--notes-url",
        "https://example.org/1.0.0",
        "--tested-from",
        &"ab".repeat(32),
        "--published-at-ns",
        "1700000000000000000",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    std::fs::write(dir.path("manifest.did"), stdout(&output)).unwrap();
    wasm
}

fn verify(dir: &Dir, signers: &[String], threshold: &str, wasm: Option<&str>) -> Output {
    let manifest = dir.path("manifest.did");
    let mut args = vec!["verify"];
    for signer in signers {
        args.extend(["--signer", signer]);
    }
    args.extend(["--threshold", threshold]);
    if let Some(wasm) = wasm {
        args.extend(["--wasm", wasm]);
    }
    args.push(&manifest);
    tool(&args)
}

#[test]
fn two_of_three_signers_sign_a_manifest() {
    let dir = Dir::new("two-of-three");
    let signers = dir.keys(3);
    let wasm = new_manifest(&dir);
    let manifest = dir.manifest();
    assert_eq!(manifest.version, "1.0.0");
    assert_eq!(manifest.chunk_hashes.len(), 3);
    assert_eq!(manifest.tested_from[0].as_slice(), [0xab; 32]);
    assert_eq!(manifest.published_at_ns, 1_700_000_000_000_000_000);

    let unsigned = verify(&dir, &signers, "2", Some(&wasm));
    assert_eq!(unsigned.status.code(), Some(1));
    assert!(stdout(&unsigned).contains("Signed by 0 of the 3 keys; 2 needed."));
    assert!(stderr(&unsigned).contains("has 0 of the 2 signatures it needs"));

    let manifest_path = dir.path("manifest.did");
    let first = tool(&[
        "sign",
        "--key",
        &dir.path("key-1.pem"),
        "--wasm",
        &wasm,
        &manifest_path,
    ]);
    assert!(first.status.success(), "{}", stderr(&first));
    assert!(stderr(&first).contains("Release 1.0.0: Wasm of 2621440 bytes"));
    assert!(verify(&dir, &signers, "2", None).status.code() == Some(1));

    let third = tool(&["sign", "--key", &dir.path("key-3.pem"), &manifest_path]);
    assert!(third.status.success(), "{}", stderr(&third));
    let signed = verify(&dir, &signers, "2", Some(&wasm));
    assert!(signed.status.success(), "{}", stderr(&signed));
    assert!(stdout(&signed).contains("Signed by 2 of the 3 keys; 2 needed."));
    assert!(stdout(&signed).contains("Its chunks are the Wasm in"));
    assert_eq!(verify(&dir, &signers, "3", None).status.code(), Some(1));

    // Signing again replaces the key's signature, and signing changes no
    // other field.
    let again = tool(&["sign", "--key", &dir.path("key-1.pem"), &manifest_path]);
    assert!(again.status.success());
    let mut signed = dir.manifest();
    assert_eq!(signed.signatures.len(), 2);
    signed.signatures.clear();
    assert_eq!(signed, manifest);
}

#[test]
fn a_manifest_for_another_wasm_is_refused() {
    let dir = Dir::new("another-wasm");
    let signers = dir.keys(3);
    new_manifest(&dir);
    let other = dir.wasm("other.wasm", 1);
    let manifest_path = dir.path("manifest.did");
    let before = std::fs::read(&manifest_path).unwrap();

    let refused = tool(&[
        "sign",
        "--key",
        &dir.path("key-1.pem"),
        "--wasm",
        &other,
        &manifest_path,
    ]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        stderr(&refused).contains("does not name the Wasm"),
        "{}",
        stderr(&refused)
    );
    assert!(stderr(&refused).contains("HashMismatch { index: 0 }"));
    assert_eq!(std::fs::read(&manifest_path).unwrap(), before);

    for key in ["key-1.pem", "key-2.pem"] {
        assert!(
            tool(&["sign", "--key", &dir.path(key), &manifest_path])
                .status
                .success()
        );
    }
    let mismatch = verify(&dir, &signers, "2", Some(&other));
    assert_eq!(mismatch.status.code(), Some(1));
    assert!(stderr(&mismatch).contains("does not name the Wasm"));
}

#[test]
fn a_signature_by_another_key_or_over_other_text_counts_for_nothing() {
    let dir = Dir::new("other-key");
    let signers = dir.keys(4);
    new_manifest(&dir);
    let manifest_path = dir.path("manifest.did");
    for key in ["key-1.pem", "key-4.pem"] {
        assert!(
            tool(&["sign", "--key", &dir.path(key), &manifest_path])
                .status
                .success()
        );
    }
    // Key 4 is not one of the three.
    let output = verify(&dir, &signers[..3], "2", None);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains("Signed by 1 of the 3 keys"));

    // A signed field changed after signing.
    assert!(
        tool(&["sign", "--key", &dir.path("key-2.pem"), &manifest_path])
            .status
            .success()
    );
    assert!(verify(&dir, &signers[..3], "2", None).status.success());
    let text = std::fs::read_to_string(&manifest_path).unwrap();
    std::fs::write(&manifest_path, text.replace("1.0.0\"", "1.0.1\"")).unwrap();
    let output = verify(&dir, &signers[..3], "2", None);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains("Release 1.0.1"));
    assert!(stdout(&output).contains("Signed by 0 of the 3 keys"));
}

#[test]
fn public_key_prints_the_key_in_hex_or_as_a_blob() {
    let dir = Dir::new("public-key");
    let signers = dir.keys(1);
    let key = dir.path("key-1.pem");
    assert_eq!(
        stdout(&tool(&["public-key", "--key", &key])).trim(),
        signers[0]
    );
    let blob = stdout(&tool(&["public-key", "--candid", "--key", &key]));
    let parsed = candid_parser::parse_idl_args(&format!("({})", blob.trim())).unwrap();
    let candid::IDLValue::Blob(bytes) = &parsed.args[0] else {
        panic!("{blob} is not a blob");
    };
    assert_eq!(hex(bytes), signers[0]);
}

#[test]
fn chunks_writes_the_arguments_of_publish_chunk() {
    let dir = Dir::new("chunks");
    let wasm = dir.wasm("wasm.wasm", 0);
    let out = dir.path("out");
    let output = tool(&[
        "chunks",
        "--wasm",
        &wasm,
        "--version",
        "1.0.0",
        "--out",
        &out,
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let bytes = std::fs::read(&wasm).unwrap();
    for (index, data) in release::chunks(&bytes).enumerate() {
        let chunk: PublishChunk =
            decode_one(&std::fs::read(format!("{out}/chunk-{index}.bin")).unwrap()).unwrap();
        assert_eq!(chunk.version, "1.0.0");
        assert_eq!(chunk.index, index as u32);
        assert_eq!(chunk.data.as_slice(), data);
    }
    assert!(!Path::new(&format!("{out}/chunk-3.bin")).exists());
}

#[test]
fn bad_input_is_refused() {
    let dir = Dir::new("bad-input");
    let wasm = dir.wasm("wasm.wasm", 0);
    let manifest = |extra: &[&str]| {
        let mut args = vec!["manifest", "--wasm", &wasm, "--version", "1.0.0"];
        args.extend(extra);
        tool(&args)
    };
    let usage = [
        tool(&[]),
        tool(&["publish"]),
        manifest(&[]),
        manifest(&["--notes-url", "https://a", "--notes-url", "https://b"]),
        manifest(&["--notes-url", "https://a", "--colour", "red"]),
        manifest(&["--notes-url", "https://a", "extra"]),
        manifest(&["--notes-url", "https://a", "--published-at-ns", "soon"]),
        manifest(&["--notes-url"]),
    ];
    for output in usage {
        assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
        assert!(stderr(&output).contains("Usage:"));
    }
    let refused = [
        manifest(&["--notes-url", "http://example.org"]),
        manifest(&["--notes-url", "https://a", "--tested-from", "abcd"]),
        tool(&[
            "manifest",
            "--wasm",
            &dir.path("missing"),
            "--version",
            "1",
            "--notes-url",
            "https://a",
        ]),
        tool(&["public-key", "--key", &wasm]),
        tool(&[
            "chunks",
            "--wasm",
            &wasm,
            "--version",
            "1/0",
            "--out",
            &dir.path("out"),
        ]),
    ];
    for output in refused {
        assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
        assert!(!stderr(&output).contains("Usage:"));
    }
}
