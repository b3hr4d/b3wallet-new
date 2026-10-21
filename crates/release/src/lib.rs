//! Wallet releases: the manifest that names a release, the hash its signers
//! sign, and the checks a release channel and a wallet run before they trust
//! one. Both run the same checks, so a manifest the channel publishes is one a
//! wallet with the same signer keys accepts.
//!
//! A release's Wasm is cut into chunks of `CHUNK_SIZE` bytes, the last one
//! shorter, so one Wasm has exactly one list of chunk hashes.

pub mod channel;

use candid::{CandidType, encode_one};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::Deserialize;
use serde_bytes::ByteBuf;
use sha2::{Digest, Sha256};

/// The size of every chunk but the last, which is the most the IC's chunk
/// store takes at once.
pub const CHUNK_SIZE: usize = 1 << 20;
/// The most chunks a release has: the IC's chunk store holds 100.
pub const MAX_CHUNKS: usize = 100;
pub const MAX_VERSION_LEN: usize = 64;
pub const MAX_TESTED_FROM: usize = 64;
pub const MAX_NOTES_URL_LEN: usize = 512;
/// The most signatures a manifest carries, and the most signer keys a
/// channel or a wallet pins.
pub const MAX_SIGNERS: usize = 16;

/// An Ed25519 signature by one release signer over the manifest's
/// `signing_hash`.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ReleaseSignature {
    pub signer: ByteBuf,
    pub signature: ByteBuf,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ReleaseManifest {
    pub version: String,
    pub wasm_sha256: ByteBuf,
    pub wasm_size: u64,
    /// One SHA-256 per chunk, in install order.
    pub chunk_hashes: Vec<ByteBuf>,
    /// Module hashes this release was tested to upgrade from.
    pub tested_from: Vec<ByteBuf>,
    pub notes_url: String,
    pub published_at_ns: u64,
    pub signatures: Vec<ReleaseSignature>,
}

impl ReleaseManifest {
    /// The unsigned manifest of `wasm`.
    pub fn new(
        version: String,
        wasm: &[u8],
        tested_from: Vec<[u8; 32]>,
        notes_url: String,
        published_at_ns: u64,
    ) -> ReleaseManifest {
        ReleaseManifest {
            version,
            wasm_sha256: ByteBuf::from(sha256(wasm)),
            wasm_size: wasm.len() as u64,
            chunk_hashes: chunks(wasm)
                .map(|chunk| ByteBuf::from(sha256(chunk)))
                .collect(),
            tested_from: tested_from.into_iter().map(ByteBuf::from).collect(),
            notes_url,
            published_at_ns,
            signatures: Vec::new(),
        }
    }

    /// Adds `key`'s signature, in place of one it made before.
    pub fn sign(&mut self, key: &SigningKey) {
        let signer = key.verifying_key().to_bytes();
        let signature = key.sign(&self.signing_hash()).to_bytes();
        self.signatures
            .retain(|existing| existing.signer.as_slice() != signer);
        self.signatures.push(ReleaseSignature {
            signer: ByteBuf::from(signer),
            signature: ByteBuf::from(signature),
        });
    }

    /// What the signers sign: SHA-256 of the manifest's Candid encoding with
    /// no signatures, as `encode_one` writes it. Another Candid encoder may
    /// lay out the type table differently, so signers sign with this crate.
    pub fn signing_hash(&self) -> [u8; 32] {
        let unsigned = ReleaseManifest {
            signatures: Vec::new(),
            ..self.clone()
        };
        sha256(&encode_one(&unsigned).expect("a manifest encodes as Candid"))
    }

    /// Checks every field's form and limit. It reads no signature and no
    /// chunk.
    pub fn check_form(&self) -> Result<(), String> {
        check_version(&self.version)?;
        if self.wasm_sha256.len() != 32 {
            return Err("wasm_sha256 must be 32 bytes".to_string());
        }
        if self.wasm_size == 0 || self.wasm_size > (MAX_CHUNKS * CHUNK_SIZE) as u64 {
            return Err(format!(
                "wasm_size must be between 1 and {} bytes",
                MAX_CHUNKS * CHUNK_SIZE
            ));
        }
        let chunks = chunk_count(self.wasm_size);
        if self.chunk_hashes.len() != chunks {
            return Err(format!(
                "a Wasm of {} bytes has {chunks} chunks of at most {CHUNK_SIZE} bytes, not {}",
                self.wasm_size,
                self.chunk_hashes.len()
            ));
        }
        if let Some(index) = self.chunk_hashes.iter().position(|hash| hash.len() != 32) {
            return Err(format!("chunk hash {index} must be 32 bytes"));
        }
        if self.tested_from.len() > MAX_TESTED_FROM {
            return Err(format!(
                "tested_from must have at most {MAX_TESTED_FROM} module hashes"
            ));
        }
        if let Some(index) = self.tested_from.iter().position(|hash| hash.len() != 32) {
            return Err(format!("tested_from hash {index} must be 32 bytes"));
        }
        check_notes_url(&self.notes_url)?;
        if self.signatures.len() > MAX_SIGNERS {
            return Err(format!("a manifest has at most {MAX_SIGNERS} signatures"));
        }
        if let Some(index) = self
            .signatures
            .iter()
            .position(|s| s.signer.len() != 32 || s.signature.len() != 64)
        {
            return Err(format!(
                "signature {index} must name a 32-byte key and hold 64 bytes"
            ));
        }
        Ok(())
    }

    /// The size chunk `index` must have.
    pub fn chunk_size(&self, index: usize) -> usize {
        let start = (index * CHUNK_SIZE) as u64;
        self.wasm_size.saturating_sub(start).min(CHUNK_SIZE as u64) as usize
    }

    /// Checks one chunk against its hash and its size.
    pub fn check_chunk(&self, index: usize, data: &[u8]) -> Result<(), ChunkError> {
        let expected = self
            .chunk_hashes
            .get(index)
            .ok_or(ChunkError::WasmMismatch)?;
        if sha256(data) != expected.as_slice() {
            return Err(ChunkError::HashMismatch {
                index: index as u32,
            });
        }
        // The chunk is the one the signers named, but the manifest cuts the
        // Wasm in another place than every manifest must.
        if data.len() != self.chunk_size(index) {
            return Err(ChunkError::WasmMismatch);
        }
        Ok(())
    }

    /// Checks every chunk, in order, then the Wasm they make up together.
    /// `chunk` returns chunk `index`, or `None` when there is none.
    pub fn check_chunks<C: AsRef<[u8]>>(
        &self,
        mut chunk: impl FnMut(usize) -> Option<C>,
    ) -> Result<(), ChunkError> {
        let mut wasm = Sha256::new();
        for index in 0..self.chunk_hashes.len() {
            let data = chunk(index).ok_or(ChunkError::Missing {
                index: index as u32,
            })?;
            self.check_chunk(index, data.as_ref())?;
            wasm.update(data.as_ref());
        }
        if wasm.finalize().as_slice() != self.wasm_sha256.as_slice() {
            return Err(ChunkError::WasmMismatch);
        }
        Ok(())
    }

    /// Checks a whole Wasm: its size, then its chunks. `check_chunks` alone
    /// never sees bytes past the last chunk the manifest names.
    pub fn check_wasm(&self, wasm: &[u8]) -> Result<(), ChunkError> {
        if wasm.len() as u64 != self.wasm_size {
            return Err(ChunkError::WasmMismatch);
        }
        self.check_chunks(|index| chunks(wasm).nth(index))
    }
}

/// Why a release's chunks do not make up the Wasm its manifest names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChunkError {
    Missing {
        index: u32,
    },
    HashMismatch {
        index: u32,
    },
    /// Every chunk matches its hash, but the chunks are not cut at
    /// `CHUNK_SIZE`, or together are not the Wasm of `wasm_sha256`; or a
    /// whole Wasm is not `wasm_size` bytes.
    WasmMismatch,
}

/// The number of chunks of a Wasm of `wasm_size` bytes.
pub fn chunk_count(wasm_size: u64) -> usize {
    wasm_size.div_ceil(CHUNK_SIZE as u64) as usize
}

/// `wasm` cut into its chunks.
pub fn chunks(wasm: &[u8]) -> impl Iterator<Item = &[u8]> {
    wasm.chunks(CHUNK_SIZE)
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// A version names a release in URLs and stable-memory keys: 1 to 64 ASCII
/// letters, digits and `.`, `+`, `-` or `_`.
pub fn check_version(version: &str) -> Result<(), String> {
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-' | '_');
    if version.is_empty() || version.len() > MAX_VERSION_LEN || !version.chars().all(allowed) {
        return Err(format!(
            "a version must be 1 to {MAX_VERSION_LEN} ASCII letters, digits and . + - _"
        ));
    }
    Ok(())
}

/// The release notes are a page the wallet links to, so only an https URL
/// with no spaces or control characters.
fn check_notes_url(url: &str) -> Result<(), String> {
    let printable = url.chars().all(|c| c.is_ascii_graphic());
    if !url.starts_with("https://") || url.len() > MAX_NOTES_URL_LEN || !printable {
        return Err(format!(
            "notes_url must be an https URL of at most {MAX_NOTES_URL_LEN} printable ASCII characters"
        ));
    }
    Ok(())
}

/// The release signer keys and how many of them must sign.
#[derive(Clone, Debug)]
pub struct Signers {
    keys: Vec<VerifyingKey>,
    threshold: u8,
}

/// A manifest signed by fewer pinned keys than the threshold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotEnoughSignatures {
    pub have: u8,
    pub need: u8,
}

impl Signers {
    /// Refuses a key that is not an Ed25519 public key, a repeated key, more
    /// than `MAX_SIGNERS` keys, and a threshold outside 1 to the number of
    /// keys.
    pub fn new<K: AsRef<[u8]>>(keys: &[K], threshold: u8) -> Result<Signers, String> {
        if keys.len() > MAX_SIGNERS {
            return Err(format!("at most {MAX_SIGNERS} release signers"));
        }
        let mut parsed = Vec::with_capacity(keys.len());
        for (index, key) in keys.iter().enumerate() {
            let key = public_key(key.as_ref()).ok_or_else(|| {
                format!("release signer key {index} must be an Ed25519 public key")
            })?;
            if parsed.contains(&key) {
                return Err("release signers must not repeat a key".to_string());
            }
            parsed.push(key);
        }
        if threshold == 0 || usize::from(threshold) > parsed.len() {
            return Err(format!(
                "the release threshold must be between 1 and the number of release signers ({})",
                parsed.len()
            ));
        }
        Ok(Signers {
            keys: parsed,
            threshold,
        })
    }

    pub fn threshold(&self) -> u8 {
        self.threshold
    }

    /// How many of the keys signed the manifest. A key counts once however
    /// often it signed; a signature by another key, or one that does not
    /// verify, counts for nothing.
    pub fn count(&self, manifest: &ReleaseManifest) -> u8 {
        let hash = manifest.signing_hash();
        let mut signed = vec![false; self.keys.len()];
        for signature in &manifest.signatures {
            let Some(index) = self
                .keys
                .iter()
                .position(|key| key.as_bytes() == signature.signer.as_slice())
            else {
                continue;
            };
            if !signed[index] && verify(&self.keys[index], &hash, &signature.signature) {
                signed[index] = true;
            }
        }
        signed.iter().filter(|&&signed| signed).count() as u8
    }

    /// Accepts a manifest signed by at least `threshold` of the keys.
    pub fn check(&self, manifest: &ReleaseManifest) -> Result<(), NotEnoughSignatures> {
        let have = self.count(manifest);
        if have < self.threshold {
            return Err(NotEnoughSignatures {
                have,
                need: self.threshold,
            });
        }
        Ok(())
    }
}

/// Decodes a public key, refusing bytes that are not a curve point and the
/// weak keys of small order, for which a signature proves nothing.
pub fn public_key(bytes: &[u8]) -> Option<VerifyingKey> {
    let key = VerifyingKey::from_bytes(bytes.try_into().ok()?).ok()?;
    (!key.is_weak()).then_some(key)
}

/// Strict verification: canonical encodings only, and no small-order
/// signature point, so one signature cannot be reshaped into another.
fn verify(key: &VerifyingKey, message: &[u8], signature: &[u8]) -> bool {
    let Ok(signature) = Signature::from_slice(signature) else {
        return false;
    };
    key.verify_strict(message, &signature).is_ok()
}

#[cfg(test)]
mod tests;
