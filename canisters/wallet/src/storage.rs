//! Stable memory. Every store lives in its own virtual memory of one
//! `MemoryManager`, so state is written in place and an upgrade never
//! serializes the heap. The memory ids are listed in `docs/design.md`; an id
//! is never reused or renumbered, even after its store goes away.
//!
//! Stored values are Candid. A release may add a field to a stored record only
//! as an `opt` field, so the values an earlier release wrote still decode.

use crate::actions::StoredPending;
use crate::governance::Governance;
use crate::{ActionId, ActionState, InitArgs, Member, Policy, Role};
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
const POLICY_MEMORY: MemoryId = MemoryId::new(1);
const MEMBERS_MEMORY: MemoryId = MemoryId::new(2);
const PENDING_MEMORY: MemoryId = MemoryId::new(4);
const OPEN_MEMORY: MemoryId = MemoryId::new(10);

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

candid_storable!(Config, Policy, StoredMember, StoredPending);

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

    static POLICY: RefCell<StableCell<Option<Policy>, Memory>> =
        RefCell::new(StableCell::init(memory(POLICY_MEMORY), None));

    static MEMBERS: RefCell<StableBTreeMap<Principal, StoredMember, Memory>> =
        RefCell::new(StableBTreeMap::init(memory(MEMBERS_MEMORY)));

    static PENDING: RefCell<StableBTreeMap<ActionId, StoredPending, Memory>> =
        RefCell::new(StableBTreeMap::init(memory(PENDING_MEMORY)));

    /// The ids of the actions not yet executed, rejected, expired or failed,
    /// so finding them never reads the whole history.
    static OPEN: RefCell<StableBTreeMap<ActionId, (), Memory>> =
        RefCell::new(StableBTreeMap::init(memory(OPEN_MEMORY)));
}

fn memory(id: MemoryId) -> Memory {
    MEMORY_MANAGER.with(|manager| manager.get(id))
}

/// Stores the configuration at install.
pub fn set_config(config: Config) {
    CONFIG.with_borrow_mut(|cell| cell.set(Some(config)));
}

/// Stores the policy at install; afterwards only `save_governance` changes it.
pub fn set_policy(policy: Policy) {
    POLICY.with_borrow_mut(|cell| cell.set(Some(policy)));
}

pub fn policy() -> Policy {
    POLICY
        .with_borrow(|cell| cell.get().clone())
        .expect("an installed wallet has a policy")
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

/// The members and the policy, read into the heap. A wallet has at most
/// `MAX_MEMBERS` members, so this stays small.
pub fn governance() -> Governance {
    Governance {
        members: MEMBERS
            .with_borrow(|members| members.iter().map(|entry| entry.into_pair()).collect()),
        policy: policy(),
    }
}

/// Writes back what an applied action changed.
pub fn save_governance(governance: &Governance) {
    MEMBERS.with_borrow_mut(|members| {
        let gone: Vec<Principal> = members
            .keys()
            .filter(|id| !governance.members.contains_key(id))
            .collect();
        for id in gone {
            members.remove(&id);
        }
        for (id, member) in &governance.members {
            if members.get(id).as_ref() != Some(member) {
                members.insert(*id, member.clone());
            }
        }
    });
    if policy() != governance.policy {
        set_policy(governance.policy.clone());
    }
}

/// Stores a new action under the next id. Ids start at 1 and only grow, and
/// actions are never deleted, so an id never names another action.
pub fn add_action(pending: StoredPending) -> ActionId {
    let id = PENDING.with_borrow_mut(|actions| {
        let id = actions.last_key_value().map_or(1, |(last, _)| last + 1);
        actions.insert(id, pending);
        id
    });
    OPEN.with_borrow_mut(|open| open.insert(id, ()));
    id
}

pub fn action(id: ActionId) -> Option<StoredPending> {
    PENDING.with_borrow(|actions| actions.get(&id))
}

pub fn set_action(id: ActionId, pending: StoredPending) {
    let closed = !matches!(pending.state, ActionState::Waiting | ActionState::Ready);
    PENDING.with_borrow_mut(|actions| actions.insert(id, pending));
    if closed {
        OPEN.with_borrow_mut(|open| open.remove(&id));
    }
}

/// The actions open at `now`, oldest first.
pub fn open_actions(now: u64) -> Vec<(ActionId, StoredPending)> {
    let ids: Vec<ActionId> = OPEN.with_borrow(|open| open.keys().collect());
    ids.into_iter()
        .map(|id| (id, action(id).expect("an open id names a stored action")))
        .filter(|(_, pending)| pending.is_open(now))
        .collect()
}

/// Writes down the expiry of every action that expired by `now`, which also
/// takes it out of the open ids. Run before adding an action, it keeps the
/// open ids to the open actions and one more.
pub fn close_expired(now: u64) {
    let ids: Vec<ActionId> = OPEN.with_borrow(|open| open.keys().collect());
    for id in ids {
        let mut pending = action(id).expect("an open id names a stored action");
        if !pending.is_open(now) {
            pending.expire(now);
            set_action(id, pending);
        }
    }
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
    if POLICY.with_borrow(|cell| cell.get().is_none()) {
        return Err("stable memory holds no policy".to_string());
    }
    MEMBERS.with_borrow(|members| members.values().for_each(drop));
    PENDING.with_borrow(|actions| actions.values().for_each(drop));
    if let Some(id) = OPEN.with_borrow(|open| open.keys().find(|id| action(*id).is_none())) {
        return Err(format!("open action {id} is not stored"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Action;
    use crate::actions::APPROVAL_WINDOW_NS;
    use crate::policy::{Terms, default_policy};

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
    fn check_fails_without_a_policy() {
        set_config(config_value());
        assert!(check().unwrap_err().contains("no policy"));
    }

    #[test]
    fn check_passes_once_configured_and_reads_the_members() {
        set_config(config_value());
        set_policy(default_policy());
        insert_member(Principal::self_authenticating([3; 32]), stored_member());
        add_action(action_at(5));
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

    fn action_at(proposed_at_ns: u64) -> StoredPending {
        StoredPending::new(
            Action::RemoveMember(Principal::self_authenticating([3; 32])),
            Principal::self_authenticating([3; 32]),
            vec![],
            Terms {
                quorum: 1,
                timelock_ns: 0,
            },
            APPROVAL_WINDOW_NS,
            proposed_at_ns,
        )
    }

    #[test]
    fn action_ids_start_at_one_and_grow() {
        assert_eq!(add_action(action_at(1)), 1);
        assert_eq!(add_action(action_at(2)), 2);
        assert_eq!(action(2), Some(action_at(2)));
        assert_eq!(action(3), None);
    }

    #[test]
    fn open_actions_are_the_waiting_and_ready_ones_not_yet_expired() {
        let now = 30 * APPROVAL_WINDOW_NS;
        let expired = add_action(action_at(now - APPROVAL_WINDOW_NS));
        let open = add_action(action_at(now - 1));
        let rejected = add_action(action_at(now));
        let mut closed = action_at(now);
        closed.state = ActionState::Rejected(Principal::anonymous());
        set_action(rejected, closed);
        let newest = add_action(action_at(now));

        let ids = |open: Vec<(ActionId, StoredPending)>| -> Vec<ActionId> {
            open.into_iter().map(|(id, _)| id).collect()
        };
        assert_eq!(ids(open_actions(now)), [open, newest]);
        assert_eq!(OPEN.with_borrow(|open| open.len()), 3);

        close_expired(now);
        assert_eq!(action(expired).unwrap().state, ActionState::Expired);
        assert_eq!(
            OPEN.with_borrow(|open| open.keys().collect::<Vec<_>>()),
            [open, newest]
        );
        assert_eq!(ids(open_actions(now)), [open, newest]);
    }

    #[test]
    fn check_fails_when_an_open_id_has_no_action() {
        set_config(config_value());
        set_policy(default_policy());
        OPEN.with_borrow_mut(|open| open.insert(3, ()));
        assert_eq!(check(), Err("open action 3 is not stored".to_string()));
    }

    #[test]
    fn saving_governance_writes_members_and_policy() {
        let (kept, removed, added) = (
            Principal::self_authenticating([3; 32]),
            Principal::self_authenticating([4; 32]),
            Principal::self_authenticating([5; 32]),
        );
        set_policy(default_policy());
        insert_member(kept, stored_member());
        insert_member(removed, stored_member());

        let mut governance = governance();
        governance.members.remove(&removed);
        governance.members.insert(added, stored_member());
        governance.policy.rules[0].quorum = 2;
        save_governance(&governance);

        assert_eq!(member(removed), None);
        assert_eq!(member(added), Some(stored_member()));
        assert_eq!(member(kept), Some(stored_member()));
        assert_eq!(policy(), governance.policy);
        assert_eq!(super::governance(), governance);
    }
}
