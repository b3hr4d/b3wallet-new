//! Stable memory, one virtual memory per store, as in the wallet. The memory
//! ids are listed in `docs/design.md`; an id is never reused or renumbered.
//! Stored values are Candid, and a release adds a field only as `opt`.

use crate::types::FactoryConfig;
use candid::{CandidType, Principal, decode_one, encode_one};
use ic_stable_structures::memory_manager::{MemoryId, MemoryManager, VirtualMemory};
use ic_stable_structures::storable::Bound;
use ic_stable_structures::{DefaultMemoryImpl, StableBTreeMap, StableCell, Storable};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_bytes::ByteBuf;
use std::borrow::Cow;
use std::cell::RefCell;

type Memory = VirtualMemory<DefaultMemoryImpl>;

const CONFIG_MEMORY: MemoryId = MemoryId::new(0);
const HAND_OVERS_MEMORY: MemoryId = MemoryId::new(1);

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

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
struct StoredConfig {
    config: FactoryConfig,
}

/// A canister the factory created and still controls.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct HandOver {
    /// Who paid for it, and gets it if the release does not install.
    pub creator: Principal,
    /// The module of the release it was created for: a canister that runs it
    /// is a wallet, and gets itself.
    pub wasm_sha256: ByteBuf,
    pub created_at_ns: u64,
}

candid_storable!(StoredConfig, HandOver);

fn to_candid<T: CandidType>(value: &T) -> Vec<u8> {
    encode_one(value).expect("a stored value encodes as Candid")
}

// A value that does not decode traps the call, which rolls back its changes.
fn from_candid<T: CandidType + DeserializeOwned>(bytes: &[u8]) -> T {
    decode_one(bytes).unwrap_or_else(|e| panic!("a stored value does not decode: {e}"))
}

thread_local! {
    static MEMORY_MANAGER: MemoryManager<DefaultMemoryImpl> =
        MemoryManager::init(DefaultMemoryImpl::default());

    static CONFIG: RefCell<StableCell<Option<StoredConfig>, Memory>> =
        RefCell::new(StableCell::init(memory(CONFIG_MEMORY), None));

    /// The canisters the factory created and has not handed over yet: those
    /// it is still installing, and those whose hand-over failed and waits for
    /// the timer.
    static HAND_OVERS: RefCell<StableBTreeMap<Principal, HandOver, Memory>> =
        RefCell::new(StableBTreeMap::init(memory(HAND_OVERS_MEMORY)));
}

fn memory(id: MemoryId) -> Memory {
    MEMORY_MANAGER.with(|manager| manager.get(id))
}

pub fn set_config(config: FactoryConfig) {
    CONFIG.with_borrow_mut(|cell| cell.set(Some(StoredConfig { config })));
}

pub fn config() -> FactoryConfig {
    CONFIG
        .with_borrow(|cell| cell.get().clone())
        .expect("an installed factory has a configuration")
        .config
}

pub fn owe_hand_over(canister: Principal, hand_over: HandOver) {
    HAND_OVERS.with_borrow_mut(|hand_overs| hand_overs.insert(canister, hand_over));
}

pub fn handed_over(canister: Principal) {
    HAND_OVERS.with_borrow_mut(|hand_overs| hand_overs.remove(&canister));
}

/// The canisters the factory still controls, oldest first by id.
pub fn hand_overs() -> Vec<(Principal, HandOver)> {
    HAND_OVERS.with_borrow(|hand_overs| hand_overs.iter().map(|e| e.into_pair()).collect())
}

/// Reads every stored value, so an upgrade that cannot read the state traps
/// in `post_upgrade` and the IC keeps the running version.
pub fn check() -> Result<(), String> {
    if CONFIG.with_borrow(|cell| cell.get().is_none()) {
        return Err("stable memory holds no configuration".to_string());
    }
    config()
        .validate()
        .map_err(|e| format!("the stored configuration is not valid: {e}"))?;
    HAND_OVERS.with_borrow(|hand_overs| hand_overs.values().for_each(drop));
    Ok(())
}
