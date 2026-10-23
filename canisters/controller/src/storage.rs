//! Stable memory, one virtual memory per store, as in the wallet. The memory
//! ids are listed in `docs/design.md`; an id is never reused or renumbered.
//! The controller is never upgraded, so its state is only ever read by the
//! code that wrote it; stored values are Candid all the same.

use crate::operations::StoredOperation;
use candid::{CandidType, Principal, decode_one, encode_one};
use ic_stable_structures::memory_manager::{MemoryId, MemoryManager, VirtualMemory};
use ic_stable_structures::storable::Bound;
use ic_stable_structures::{DefaultMemoryImpl, StableBTreeMap, StableCell, Storable};
use recovery::Policy;
use recovery::controller::ExecutionStep;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::borrow::Cow;
use std::cell::RefCell;

type Memory = VirtualMemory<DefaultMemoryImpl>;

const CONFIG_MEMORY: MemoryId = MemoryId::new(0);
const OPERATIONS_MEMORY: MemoryId = MemoryId::new(1);
const OPEN_MEMORY: MemoryId = MemoryId::new(2);
const CHUNKS_MEMORY: MemoryId = MemoryId::new(3);
const EXECUTION_MEMORY: MemoryId = MemoryId::new(4);

/// What the controller serves and under which policy.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub wallet: Principal,
    pub policy: Policy,
    /// 1 at install; each executed `SetPolicy` adds one.
    pub policy_version: u64,
    pub next_nonce: u64,
    pub replaced_by: Option<Principal>,
    /// The controller stopped the wallet and has not started it again.
    pub wallet_stopped: bool,
}

/// The execution in flight, written before every call it makes.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StoredExecution {
    pub operation: Option<u64>,
    pub step: ExecutionStep,
    pub since_ns: u64,
}

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

candid_storable!(Config, StoredOperation, StoredExecution);

fn to_candid<T: CandidType>(value: &T) -> Vec<u8> {
    encode_one(value).expect("a stored value encodes as Candid")
}

// A value that does not decode traps the call, which rolls back its changes.
fn from_candid<T: CandidType + DeserializeOwned>(bytes: &[u8]) -> T {
    decode_one(bytes).unwrap_or_else(|e| panic!("a stored value does not decode: {e}"))
}

/// A chunk's key: its operation and its index, big-endian, so an
/// operation's chunks sort together by index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ChunkKey(u64, u32);

impl Storable for ChunkKey {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        let mut bytes = self.0.to_be_bytes().to_vec();
        bytes.extend_from_slice(&self.1.to_be_bytes());
        Cow::Owned(bytes)
    }

    fn into_bytes(self) -> Vec<u8> {
        self.to_bytes().into_owned()
    }

    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        let operation = u64::from_be_bytes(bytes[..8].try_into().expect("8 bytes"));
        let index = u32::from_be_bytes(bytes[8..12].try_into().expect("4 bytes"));
        ChunkKey(operation, index)
    }

    const BOUND: Bound = Bound::Bounded {
        max_size: 12,
        is_fixed_size: true,
    };
}

thread_local! {
    static MEMORY_MANAGER: MemoryManager<DefaultMemoryImpl> =
        MemoryManager::init(DefaultMemoryImpl::default());

    static CONFIG: RefCell<StableCell<Option<Config>, Memory>> =
        RefCell::new(StableCell::init(memory(CONFIG_MEMORY), None));

    /// Every operation ever proposed, by nonce; never deleted, so an id
    /// never names another operation.
    static OPERATIONS: RefCell<StableBTreeMap<u64, StoredOperation, Memory>> =
        RefCell::new(StableBTreeMap::init(memory(OPERATIONS_MEMORY)));

    /// The operations still open.
    static OPEN: RefCell<StableBTreeMap<u64, (), Memory>> =
        RefCell::new(StableBTreeMap::init(memory(OPEN_MEMORY)));

    /// The uploaded chunks of open `InstallCode` operations.
    static CHUNKS: RefCell<StableBTreeMap<ChunkKey, Vec<u8>, Memory>> =
        RefCell::new(StableBTreeMap::init(memory(CHUNKS_MEMORY)));

    static EXECUTION: RefCell<StableCell<Option<StoredExecution>, Memory>> =
        RefCell::new(StableCell::init(memory(EXECUTION_MEMORY), None));
}

fn memory(id: MemoryId) -> Memory {
    MEMORY_MANAGER.with(|manager| manager.get(id))
}

pub fn set_config(config: Config) {
    CONFIG.with_borrow_mut(|cell| cell.set(Some(config)));
}

pub fn config() -> Config {
    CONFIG
        .with_borrow(|cell| cell.get().clone())
        .unwrap_or_else(|| ic_cdk::trap("the controller has no configuration"))
}

pub fn operation(id: u64) -> Option<StoredOperation> {
    OPERATIONS.with_borrow(|operations| operations.get(&id))
}

/// Stores an operation, and keeps the set of open ones in step with it.
pub fn set_operation(id: u64, operation: StoredOperation) {
    let open = operation.is_open();
    OPERATIONS.with_borrow_mut(|operations| operations.insert(id, operation));
    OPEN.with_borrow_mut(|set| {
        if open {
            set.insert(id, ());
        } else {
            set.remove(&id);
        }
    });
    if !open {
        delete_chunks(id);
    }
}

/// The open operations, oldest first.
pub fn open_operations() -> Vec<(u64, StoredOperation)> {
    let ids: Vec<u64> = OPEN.with_borrow(|set| set.keys().collect());
    ids.into_iter()
        .map(|id| (id, operation(id).expect("an open operation is stored")))
        .collect()
}

/// Newest first: up to `limit` operations before `before`, or from the
/// newest.
pub fn operations_page(before: Option<u64>, limit: usize) -> Vec<(u64, StoredOperation)> {
    OPERATIONS.with_borrow(|operations| {
        let end = before.unwrap_or(u64::MAX);
        operations
            .range(..end)
            .rev()
            .take(limit)
            .map(|entry| (*entry.key(), entry.value()))
            .collect()
    })
}

/// The operations proposed at or after `since_ns`, newest first: ids grow
/// with time.
pub fn proposed_since(since_ns: u64) -> Vec<StoredOperation> {
    OPERATIONS.with_borrow(|operations| {
        operations
            .iter()
            .rev()
            .map(|entry| entry.value())
            .take_while(|stored| stored.proposed_at_ns >= since_ns)
            .collect()
    })
}

pub fn chunk(operation: u64, index: u32) -> Option<Vec<u8>> {
    CHUNKS.with_borrow(|chunks| chunks.get(&ChunkKey(operation, index)))
}

pub fn has_chunk(operation: u64, index: u32) -> bool {
    CHUNKS.with_borrow(|chunks| chunks.contains_key(&ChunkKey(operation, index)))
}

/// How many chunks the controller holds for all open operations.
pub fn chunk_count() -> u64 {
    CHUNKS.with_borrow(|chunks| chunks.len())
}

pub fn set_chunk(operation: u64, index: u32, data: Vec<u8>) {
    CHUNKS.with_borrow_mut(|chunks| chunks.insert(ChunkKey(operation, index), data));
}

fn delete_chunks(operation: u64) {
    CHUNKS.with_borrow_mut(|chunks| {
        let keys: Vec<ChunkKey> = chunks
            .keys_range(ChunkKey(operation, 0)..=ChunkKey(operation, u32::MAX))
            .collect();
        for key in keys {
            chunks.remove(&key);
        }
    });
}

pub fn execution() -> Option<StoredExecution> {
    EXECUTION.with_borrow(|cell| cell.get().clone())
}

pub fn set_execution(execution: Option<StoredExecution>) {
    EXECUTION.with_borrow_mut(|cell| cell.set(execution));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_keys_sort_by_operation_then_index() {
        let mut keys = vec![
            ChunkKey(1, 256),
            ChunkKey(2, 0),
            ChunkKey(1, 1),
            ChunkKey(0, u32::MAX),
        ];
        keys.sort_by_key(|key| key.to_bytes().into_owned());
        assert_eq!(
            keys,
            vec![
                ChunkKey(0, u32::MAX),
                ChunkKey(1, 1),
                ChunkKey(1, 256),
                ChunkKey(2, 0)
            ]
        );
        let key = ChunkKey(7, 9);
        assert_eq!(ChunkKey::from_bytes(key.to_bytes()), key);
    }
}
