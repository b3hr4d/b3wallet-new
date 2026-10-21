//! Stable memory, one virtual memory per store, as in the wallet. The memory
//! ids are listed in `docs/design.md`; an id is never reused or renumbered.
//! Stored values are Candid, and a release adds a field only as `opt`.

use candid::{CandidType, decode_one, encode_one};
use ic_stable_structures::memory_manager::{MemoryId, MemoryManager, VirtualMemory};
use ic_stable_structures::storable::Bound;
use ic_stable_structures::{DefaultMemoryImpl, StableBTreeMap, StableCell, Storable};
use release::channel::ChannelConfig;
use release::{MAX_VERSION_LEN, ReleaseManifest};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::borrow::Cow;
use std::cell::RefCell;

type Memory = VirtualMemory<DefaultMemoryImpl>;

const CONFIG_MEMORY: MemoryId = MemoryId::new(0);
const RELEASES_MEMORY: MemoryId = MemoryId::new(1);
const ORDER_MEMORY: MemoryId = MemoryId::new(2);
const CHUNKS_MEMORY: MemoryId = MemoryId::new(3);

macro_rules! candid_storable {
    ($($stored:ty),*) => {$(
        impl Storable for $stored {
            fn to_bytes(&self) -> Cow<'_, [u8]> {
                Cow::Owned(to_candid(self))
            }

            fn into_bytes(self) -> Vec<u8> {
                to_candid(&self)
            }

            fn from_bytes(bytes: Cow<[u8]>) -> Self {
                from_candid(&bytes)
            }

            const BOUND: Bound = Bound::Unbounded;
        }
    )*};
}

// The configuration and the manifest are the `release` crate's types, so
// they are stored inside types of this one.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
struct StoredConfig {
    config: ChannelConfig,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
struct StoredRelease {
    manifest: ReleaseManifest,
}

candid_storable!(StoredConfig, StoredRelease);

fn to_candid<T: CandidType>(value: &T) -> Vec<u8> {
    encode_one(value).expect("a stored value encodes as Candid")
}

// A value that does not decode traps the call, which rolls back its changes.
fn from_candid<T: CandidType + DeserializeOwned>(bytes: &[u8]) -> T {
    decode_one(bytes).unwrap_or_else(|e| panic!("a stored value does not decode: {e}"))
}

/// A chunk's key: its version, a zero byte, which no version holds, and its
/// index in big-endian order, so a release's chunks sort by index.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ChunkKey(Vec<u8>);

impl ChunkKey {
    fn new(version: &str, index: u32) -> ChunkKey {
        let mut key = Vec::with_capacity(version.len() + 5);
        key.extend_from_slice(version.as_bytes());
        key.push(0);
        key.extend_from_slice(&index.to_be_bytes());
        ChunkKey(key)
    }
}

impl Storable for ChunkKey {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Borrowed(&self.0)
    }

    fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        ChunkKey(bytes.into_owned())
    }

    const BOUND: Bound = Bound::Bounded {
        max_size: MAX_VERSION_LEN as u32 + 5,
        is_fixed_size: false,
    };
}

thread_local! {
    static MEMORY_MANAGER: MemoryManager<DefaultMemoryImpl> =
        MemoryManager::init(DefaultMemoryImpl::default());

    static CONFIG: RefCell<StableCell<Option<StoredConfig>, Memory>> =
        RefCell::new(StableCell::init(memory(CONFIG_MEMORY), None));

    /// The published releases, by version.
    static RELEASES: RefCell<StableBTreeMap<String, StoredRelease, Memory>> =
        RefCell::new(StableBTreeMap::init(memory(RELEASES_MEMORY)));

    /// The published versions, by the order they were published in.
    static ORDER: RefCell<StableBTreeMap<u64, String, Memory>> =
        RefCell::new(StableBTreeMap::init(memory(ORDER_MEMORY)));

    /// The chunks of published releases, and those staged for releases not
    /// published yet.
    static CHUNKS: RefCell<StableBTreeMap<ChunkKey, Vec<u8>, Memory>> =
        RefCell::new(StableBTreeMap::init(memory(CHUNKS_MEMORY)));
}

fn memory(id: MemoryId) -> Memory {
    MEMORY_MANAGER.with(|manager| manager.get(id))
}

pub fn set_config(config: ChannelConfig) {
    CONFIG.with_borrow_mut(|cell| cell.set(Some(StoredConfig { config })));
}

pub fn config() -> ChannelConfig {
    CONFIG
        .with_borrow(|cell| cell.get().clone())
        .expect("an installed channel has a configuration")
        .config
}

pub fn is_published(version: &str) -> bool {
    RELEASES.with_borrow(|releases| releases.contains_key(&version.to_string()))
}

pub fn release(version: &str) -> Option<ReleaseManifest> {
    RELEASES.with_borrow(|releases| releases.get(&version.to_string()).map(|r| r.manifest))
}

pub fn latest() -> Option<ReleaseManifest> {
    let version = ORDER.with_borrow(|order| order.last_key_value().map(|(_, version)| version))?;
    release(&version)
}

/// Every published release, in the order they were published.
pub fn releases() -> Vec<ReleaseManifest> {
    ORDER.with_borrow(|order| {
        order
            .values()
            .map(|version| release(&version).expect("a published version is stored"))
            .collect()
    })
}

pub fn chunk(version: &str, index: u32) -> Option<Vec<u8>> {
    CHUNKS.with_borrow(|chunks| chunks.get(&ChunkKey::new(version, index)))
}

pub fn stage_chunk(version: &str, index: u32, data: Vec<u8>) {
    CHUNKS.with_borrow_mut(|chunks| chunks.insert(ChunkKey::new(version, index), data));
}

/// Makes a release live, and drops the chunks staged for it past its last
/// one.
pub fn publish(manifest: ReleaseManifest) {
    let version = manifest.version.clone();
    let count = manifest.chunk_hashes.len() as u32;
    CHUNKS.with_borrow_mut(|chunks| {
        let extra: Vec<ChunkKey> = chunks
            .keys_range(ChunkKey::new(&version, count)..=ChunkKey::new(&version, u32::MAX))
            .collect();
        for key in extra {
            chunks.remove(&key);
        }
    });
    ORDER.with_borrow_mut(|order| {
        let next = order.last_key_value().map_or(0, |(last, _)| last + 1);
        order.insert(next, version.clone());
    });
    RELEASES.with_borrow_mut(|releases| releases.insert(version, StoredRelease { manifest }));
}

/// Reads every stored value, so an upgrade that cannot read the state traps
/// in `post_upgrade` and the IC keeps the running version.
pub fn check() -> Result<(), String> {
    if CONFIG.with_borrow(|cell| cell.get().is_none()) {
        return Err("stable memory holds no configuration".to_string());
    }
    // A release with tighter limits would otherwise trap on every publish.
    config()
        .validate()
        .map_err(|e| format!("the stored configuration is not valid: {e}"))?;
    RELEASES.with_borrow(|releases| releases.values().for_each(drop));
    let ordered = ORDER.with_borrow(|order| order.len());
    let stored = RELEASES.with_borrow(|releases| releases.len());
    if ordered != stored {
        return Err(format!(
            "{ordered} releases are in publish order but {stored} are stored"
        ));
    }
    if let Some(version) =
        ORDER.with_borrow(|order| order.values().find(|version| !is_published(version)))
    {
        return Err(format!("published version {version} is not stored"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_keys_sort_by_version_then_index() {
        let mut keys = vec![
            ChunkKey::new("1.0", 256),
            ChunkKey::new("1.0.1", 0),
            ChunkKey::new("1.0", 1),
            ChunkKey::new("1.0", 0),
        ];
        keys.sort();
        assert_eq!(
            keys,
            vec![
                ChunkKey::new("1.0", 0),
                ChunkKey::new("1.0", 1),
                ChunkKey::new("1.0", 256),
                ChunkKey::new("1.0.1", 0),
            ]
        );
    }

    #[test]
    fn the_longest_chunk_key_fits_its_bound() {
        let key = ChunkKey::new(&"9".repeat(MAX_VERSION_LEN), u32::MAX);
        let Bound::Bounded { max_size, .. } = ChunkKey::BOUND else {
            unreachable!()
        };
        assert_eq!(key.to_bytes().len(), max_size as usize);
    }
}
