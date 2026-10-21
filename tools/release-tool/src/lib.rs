//! What `release-tool` reads and writes: manifests as Candid text, keys as
//! PKCS#8 PEM, and chunks as the Candid arguments of `publish_chunk`.
//!
//! A manifest file is the argument of `publish_manifest` as Candid text, so
//! icp-cli sends it as it is (`--args-file`). It names every value's type, so
//! it parses without the interface file.

use candid::types::TypeEnv;
use candid::{CandidType, decode_one, encode_one};
use ed25519_dalek::SigningKey;
use ed25519_dalek::pkcs8::DecodePrivateKey;
use release::channel::PublishChunk;
use release::{ReleaseManifest, chunks};
use serde_bytes::ByteBuf;
use std::fmt::Write;

/// The manifest as the argument of `publish_manifest`, one field per line.
pub fn to_text(manifest: &ReleaseManifest) -> String {
    let blobs = |blobs: &[ByteBuf]| -> String {
        if blobs.is_empty() {
            return "vec {}".to_string();
        }
        let mut out = "vec {\n".to_string();
        for b in blobs {
            writeln!(out, "      {};", candid_blob(b)).unwrap();
        }
        out + "    }"
    };
    let mut signatures = "vec {".to_string();
    for s in &manifest.signatures {
        write!(
            signatures,
            "\n      record {{\n        signer = {};\n        signature = {};\n      }};",
            candid_blob(&s.signer),
            candid_blob(&s.signature)
        )
        .unwrap();
    }
    signatures += if manifest.signatures.is_empty() {
        "}"
    } else {
        "\n    }"
    };
    format!(
        "(\n  record {{\n    version = {};\n    wasm_sha256 = {};\n    wasm_size = {} : nat64;\n    chunk_hashes = {};\n    tested_from = {};\n    notes_url = {};\n    published_at_ns = {} : nat64;\n    signatures = {};\n  }},\n)\n",
        text(&manifest.version),
        candid_blob(&manifest.wasm_sha256),
        manifest.wasm_size,
        blobs(&manifest.chunk_hashes),
        blobs(&manifest.tested_from),
        text(&manifest.notes_url),
        manifest.published_at_ns,
        signatures,
    )
}

/// Reads a manifest written by `to_text`, or any Candid text of the same
/// value.
pub fn from_text(text: &str) -> Result<ReleaseManifest, String> {
    let args = candid_parser::parse_idl_args(text)
        .map_err(|e| format!("the manifest is not Candid text: {e}"))?;
    let bytes = args
        .to_bytes_with_types(&TypeEnv::new(), &[ReleaseManifest::ty()])
        .map_err(|e| format!("the manifest is not a ReleaseManifest: {e}"))?;
    decode_one(&bytes).map_err(|e| format!("the manifest is not a ReleaseManifest: {e}"))
}

/// A Candid text literal.
fn text(value: &str) -> String {
    let mut out = "\"".to_string();
    for c in value.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            c if c.is_ascii_graphic() || c == ' ' => out.push(c),
            c => write!(out, "\\u{{{:x}}}", c as u32).unwrap(),
        }
    }
    out + "\""
}

/// A Candid blob literal, every byte escaped.
pub fn candid_blob(bytes: &[u8]) -> String {
    let mut out = "blob \"".to_string();
    for b in bytes {
        write!(out, "\\{b:02x}").unwrap();
    }
    out + "\""
}

/// The arguments of `publish_chunk` for each chunk of `wasm`, Candid-encoded.
pub fn chunk_args(version: &str, wasm: &[u8]) -> Vec<Vec<u8>> {
    chunks(wasm)
        .enumerate()
        .map(|(index, data)| {
            encode_one(PublishChunk {
                version: version.to_string(),
                index: index as u32,
                data: ByteBuf::from(data),
            })
            .expect("a chunk encodes as Candid")
        })
        .collect()
}

/// An Ed25519 private key in PKCS#8 PEM, as `openssl genpkey -algorithm
/// ed25519` writes it.
pub fn read_key(pem: &str) -> Result<SigningKey, String> {
    SigningKey::from_pkcs8_pem(pem)
        .map_err(|e| format!("not an Ed25519 private key in PKCS#8 PEM: {e}"))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn from_hex(text: &str) -> Result<Vec<u8>, String> {
    let digits = text.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return Err(format!("{text} is not hex: odd length"));
    }
    digits
        .chunks(2)
        .map(|pair| match pair {
            [high, low] if high.is_ascii_hexdigit() && low.is_ascii_hexdigit() => {
                Ok(digit(*high) << 4 | digit(*low))
            }
            _ => Err(format!("{text} is not hex")),
        })
        .collect()
}

fn digit(hex: u8) -> u8 {
    (hex as char).to_digit(16).expect("a hex digit") as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
    use release::CHUNK_SIZE;

    fn manifest() -> ReleaseManifest {
        let wasm: Vec<u8> = (0..CHUNK_SIZE * 11 + 7).map(|i| i as u8).collect();
        let mut manifest = ReleaseManifest::new(
            "1.0.0-rc.1".to_string(),
            &wasm,
            vec![[1; 32], [2; 32]],
            "https://example.org/notes?a=\"b\"&c=\\d".to_string(),
            u64::MAX,
        );
        manifest.sign(&SigningKey::from_bytes(&[1; 32]));
        manifest.sign(&SigningKey::from_bytes(&[2; 32]));
        manifest
    }

    /// Twelve chunk hashes: more than the ten Candid's own printer shows
    /// before it cuts a vector short.
    #[test]
    fn the_text_reads_back_as_the_same_manifest() {
        let manifest = manifest();
        assert_eq!(manifest.chunk_hashes.len(), 12);
        assert_eq!(from_text(&to_text(&manifest)), Ok(manifest.clone()));

        let mut empty = manifest;
        empty.signatures.clear();
        empty.tested_from.clear();
        assert_eq!(from_text(&to_text(&empty)), Ok(empty));
    }

    #[test]
    fn the_text_reads_back_as_the_same_candid_value() {
        let manifest = manifest();
        let parsed = candid_parser::parse_idl_args(&to_text(&manifest)).unwrap();
        let encoded = candid::IDLArgs::from_bytes(&encode_one(&manifest).unwrap()).unwrap();
        assert_eq!(
            parsed
                .annotate_types(true, &TypeEnv::new(), &[ReleaseManifest::ty()])
                .unwrap(),
            encoded
        );
    }

    #[test]
    fn other_text_is_refused() {
        assert!(from_text("(record { version = \"1\" })").is_err());
        assert!(from_text("not candid").is_err());
        assert!(from_text("").is_err());
    }

    #[test]
    fn chunk_args_decode_as_publish_chunk() {
        let wasm: Vec<u8> = (0..CHUNK_SIZE + 3).map(|i| i as u8).collect();
        let args = chunk_args("2.0.0", &wasm);
        assert_eq!(args.len(), 2);
        let last: PublishChunk = decode_one(&args[1]).unwrap();
        assert_eq!(last.version, "2.0.0");
        assert_eq!(last.index, 1);
        assert_eq!(last.data.as_slice(), &wasm[CHUNK_SIZE..]);
    }

    #[test]
    fn a_pem_key_reads_back() {
        let key = SigningKey::from_bytes(&[5; 32]);
        let pem = key.to_pkcs8_pem(LineEnding::LF).unwrap();
        assert_eq!(read_key(&pem).unwrap().to_bytes(), key.to_bytes());
        assert!(read_key("-----BEGIN PRIVATE KEY-----\n-----END PRIVATE KEY-----\n").is_err());
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(
            from_hex(&hex(&[0, 1, 0xab, 0xff])),
            Ok(vec![0, 1, 0xab, 0xff])
        );
        assert_eq!(from_hex("ABff"), Ok(vec![0xab, 0xff]));
        assert!(from_hex("abc").is_err());
        assert!(from_hex("zz").is_err());
        assert!(from_hex("+1").is_err());
    }
}
