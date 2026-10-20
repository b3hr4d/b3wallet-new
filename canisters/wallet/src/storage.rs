//! Stable memory. Every store lives in its own virtual memory of one
//! `MemoryManager`, so state is written in place and an upgrade never
//! serializes the heap. The memory ids are listed in `docs/design.md`; an id
//! is never reused or renumbered, even after its store goes away.
//!
//! Stored values are Candid. A release may add a field to a stored record only
//! as an `opt` field, so the values an earlier release wrote still decode.

use crate::{InitArgs, Member, Role};
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
const MEMBERS_MEMORY: MemoryId = MemoryId::new(2);

/// What the wallet was installed with.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Present until the wallet is claimed.
    pub claim: Option<PendingClaim>,
    pub release_channel: Principal,
    pub release_signers: Vec<ByteBuf>,
    pub release_threshold: u8,
    pub factory: Option<Principal>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PendingClaim {
    pub public_key: ByteBuf,
    pub expires_at_ns: u64,
}

/// A member, stored under its principal.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StoredMember {
    pub role: Role,
    pub label: String,
    pub added_at_ns: u64,
    /// `None` for the member who claimed the wallet.
    pub added_by: Option<Principal>,
}

impl StoredMember {
    /// The member as the interface shows it.
    pub fn into_member(self, id: Principal) -> Member {
        Member {
            id,
            role: self.role,
            label: self.label,
            added_at_ns: self.added_at_ns,
            added_by: self.added_by,
        }
    }
}

impl From<InitArgs> for Config {
    fn from(args: InitArgs) -> Self {
        Config {
            claim: Some(PendingClaim {
                public_key: args.claim_public_key,
                expires_at_ns: args.claim_expires_at_ns,
            }),
            release_channel: args.release_channel,
            release_signers: args.release_signers,
            release_threshold: args.release_threshold,
            factory: args.factory,
        }
    }
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

candid_storable!(Config, StoredMember);

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

    static CONFIG: RefCell<StableCell<Option<Config>, Memory>> =
        RefCell::new(StableCell::init(memory(CONFIG_MEMORY), None));

    static MEMBERS: RefCell<StableBTreeMap<Principal, StoredMember, Memory>> =
        RefCell::new(StableBTreeMap::init(memory(MEMBERS_MEMORY)));
}

fn memory(id: MemoryId) -> Memory {
    MEMORY_MANAGER.with(|manager| manager.get(id))
}

/// Stores the configuration at install.
pub fn set_config(config: Config) {
    CONFIG.with_borrow_mut(|cell| cell.set(Some(config)));
}

pub fn config() -> Option<Config> {
    CONFIG.with_borrow(|cell| cell.get().clone())
}

pub fn is_claimed() -> bool {
    MEMBERS.with_borrow(|members| !members.is_empty())
}

pub fn member(id: Principal) -> Option<StoredMember> {
    MEMBERS.with_borrow(|members| members.get(&id))
}

/// Every member, in principal order.
pub fn members() -> Vec<Member> {
    MEMBERS.with_borrow(|members| {
        members
            .iter()
            .map(|entry| {
                let (id, member) = entry.into_pair();
                member.into_member(id)
            })
            .collect()
    })
}

#[cfg(test)]
pub fn insert_member(id: Principal, member: StoredMember) {
    MEMBERS.with_borrow_mut(|members| members.insert(id, member));
}

/// Adds the first owner and deletes the claim key, in the same message, so
/// the claim cannot be used twice.
pub fn complete_claim(owner: Principal, member: StoredMember) {
    CONFIG.with_borrow_mut(|cell| {
        let mut config = cell
            .get()
            .clone()
            .expect("a claimed wallet has a configuration");
        config.claim = None;
        cell.set(Some(config));
    });
    MEMBERS.with_borrow_mut(|members| members.insert(owner, member));
}

/// Decodes every stored value. `post_upgrade` calls it, so a release that
/// cannot read the state fails its upgrade instead of a later call.
pub fn check() -> Result<(), String> {
    if config().is_none() {
        return Err("stable memory holds no configuration".to_string());
    }
    MEMBERS.with_borrow(|members| members.values().for_each(drop));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_value() -> Config {
        Config {
            claim: Some(PendingClaim {
                public_key: ByteBuf::from(vec![1; 32]),
                expires_at_ns: 7,
            }),
            release_channel: Principal::from_slice(&[0, 0, 0, 0, 0, 0, 0, 1, 1, 1]),
            release_signers: vec![ByteBuf::from(vec![2; 32])],
            release_threshold: 1,
            factory: None,
        }
    }

    fn stored_member() -> StoredMember {
        StoredMember {
            role: Role::Owner,
            label: "laptop".to_string(),
            added_at_ns: 9,
            added_by: None,
        }
    }

    #[test]
    fn stored_values_round_trip_through_their_bytes() {
        let config = config_value();
        assert_eq!(Config::from_bytes(config.to_bytes()), config);
        let member = stored_member();
        assert_eq!(StoredMember::from_bytes(member.to_bytes()), member);
    }

    /// The rule every release follows: a field added later is `opt`, so the
    /// bytes written now still decode.
    #[test]
    fn a_later_release_can_add_optional_fields() {
        #[derive(CandidType, Deserialize, Debug)]
        struct LaterMember {
            role: Role,
            label: String,
            added_at_ns: u64,
            added_by: Option<Principal>,
            device_name: Option<String>,
        }

        let later: LaterMember = from_candid(&stored_member().to_bytes());
        assert_eq!(later.role, Role::Owner);
        assert_eq!(later.label, "laptop");
        assert_eq!(later.added_at_ns, 9);
        assert_eq!(later.added_by, None);
        assert_eq!(later.device_name, None);
    }

    #[test]
    #[should_panic(expected = "does not decode")]
    fn a_required_field_cannot_be_added() {
        #[derive(CandidType, Deserialize, Debug)]
        struct BrokenMember {
            #[allow(dead_code)]
            device_name: String,
        }

        let _: BrokenMember = from_candid(&stored_member().to_bytes());
    }

    #[test]
    fn check_fails_without_a_configuration() {
        assert!(check().unwrap_err().contains("no configuration"));
    }

    #[test]
    fn check_passes_once_configured_and_reads_the_members() {
        set_config(config_value());
        insert_member(Principal::self_authenticating([3; 32]), stored_member());
        assert_eq!(check(), Ok(()));
        assert_eq!(config(), Some(config_value()));
        assert!(is_claimed());
    }

    #[test]
    fn members_are_read_back_with_their_principal() {
        let (first, second) = (
            Principal::self_authenticating([3; 32]),
            Principal::self_authenticating([4; 32]),
        );
        assert_eq!(member(first), None);
        assert_eq!(members(), vec![]);

        insert_member(first, stored_member());
        insert_member(second, stored_member());
        assert_eq!(member(first), Some(stored_member()));
        let mut expected = vec![
            stored_member().into_member(first),
            stored_member().into_member(second),
        ];
        expected.sort_by_key(|member| member.id);
        assert_eq!(members(), expected);
    }
}
