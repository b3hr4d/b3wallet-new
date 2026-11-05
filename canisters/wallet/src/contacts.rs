//! The owners' contacts (decision 0015): a name and at most one address per
//! kind of account, behind the `SetContact` and `RemoveContact` actions.
//! Pure: `storage` keeps them, and the caller checks and applies an action.
//!
//! A name is unique among the contacts and the wallet's own account labels
//! when it is saved, and is never an address. Names are resolved again where
//! they are read (the guard, the pages and the agent's tools), since a label
//! added later can still clash.

use crate::{
    AccountKind, Action, BitcoinNetwork, Contact, ContactAddress, ContactId, ProposerRecipients,
    WalletError, btc, evm,
};
use bitcoin::Address;
use candid::{CandidType, Principal};
use icrc_ledger_types::icrc1::account::Account as LedgerAccount;
use serde::Deserialize;
use std::str::FromStr;

/// At most this many contacts, so every list of them stays small.
pub const MAX_CONTACTS: usize = 256;
/// The longest contact name, in bytes of UTF-8.
pub const MAX_NAME_BYTES: usize = 64;
/// The longest address text, in bytes.
pub const MAX_ADDRESS_BYTES: usize = 128;

/// A contact, stored under its id. A removed one keeps its id, so an id never
/// names another contact.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StoredContact {
    pub name: String,
    pub addresses: Vec<ContactAddress>,
    pub added_at_ns: u64,
    pub added_by: Principal,
    pub changed_at_ns: Option<u64>,
    pub changed_by: Option<Principal>,
    pub removed_at_ns: Option<u64>,
}

impl StoredContact {
    pub fn view(&self, id: ContactId) -> Contact {
        Contact {
            id,
            name: self.name.clone(),
            addresses: self.addresses.clone(),
            added_at_ns: self.added_at_ns,
            added_by: self.added_by,
            changed_at_ns: self.changed_at_ns,
            changed_by: self.changed_by,
        }
    }

    /// Its address for accounts of `kind`.
    pub fn address(&self, kind: AccountKind) -> Option<&str> {
        self.addresses
            .iter()
            .find(|address| address.kind == kind)
            .map(|address| address.address.as_str())
    }
}

/// A name as names are compared (candid-profile D-045): the key account
/// labels are compared by (decision 0016), so a contact's name and a label
/// are the same name exactly when the names rule says so.
pub fn name_key(name: &str) -> String {
    crate::accounts::name_key(name)
}

/// Refuses `label` for an account when a contact already has that name, so a
/// name stands for one account or one contact (decisions 0015 and 0016).
pub fn check_label_free(
    label: &str,
    contacts: &[(ContactId, StoredContact)],
) -> Result<(), WalletError> {
    let key = name_key(label);
    match contacts.iter().find(|(_, c)| name_key(&c.name) == key) {
        Some((id, _)) => Err(WalletError::InvalidArgument(format!(
            "contact {id} is already named \u{201c}{}\u{201d}: an account needs a name no contact has",
            label.trim()
        ))),
        None => Ok(()),
    }
}

/// Whether `name` is plain English, as the first version takes a contact's
/// name: printable ASCII (letters, digits, spaces and punctuation) with at
/// least one letter. Owners write requests in English, and an ASCII name needs
/// no Unicode normalization to compare, in the wallet, the guard or the pages.
// shortcut: names in other scripts are refused, not normalized. Upgrade by
// giving the wallet and the guard NFKC tables in the same change.
pub fn plain_name(name: &str) -> bool {
    name.chars().all(|c| c == ' ' || c.is_ascii_graphic())
        && name.chars().any(|c| c.is_ascii_alphabetic())
}

/// The canonical text of `text` as an address of `kind`, or why it is none.
pub fn check_address(
    kind: AccountKind,
    text: &str,
    network: BitcoinNetwork,
) -> Result<String, String> {
    if text.len() > MAX_ADDRESS_BYTES {
        return Err(format!("an address has at most {MAX_ADDRESS_BYTES} bytes"));
    }
    match kind {
        AccountKind::InternetComputer => {
            let account = LedgerAccount::from_str(text)
                .map_err(|error| format!("not an ICRC-1 account: {error}"))?;
            if account.owner == Principal::anonymous() {
                return Err("the anonymous principal cannot be sent tokens".to_string());
            }
            Ok(account.to_string())
        }
        AccountKind::Bitcoin(on) => {
            if on != network {
                return Err(format!("the wallet's Bitcoin network is {network:?}"));
            }
            let address = Address::from_str(text)
                .map_err(|error| format!("not a Bitcoin address: {error}"))?
                .require_network(btc::network(on))
                .map_err(|_| format!("not an address of Bitcoin {on:?}"))?;
            Ok(address.to_string())
        }
        AccountKind::Evm => {
            let address = evm::parse_address(text)?;
            if address == [0; 20] {
                return Err("the zero address burns what it is sent".to_string());
            }
            Ok(evm::checksummed(&address))
        }
    }
}

/// Whether `text` reads as an address of any kind, so it cannot be a name.
fn is_address(text: &str) -> bool {
    let text = text.trim();
    LedgerAccount::from_str(text).is_ok()
        || Address::from_str(text).is_ok()
        || evm::parse_address(text).is_ok()
}

/// Whether `a` and `b` are the same address of `kind`, as the ledger or the
/// chain reads them.
pub fn same_address(kind: AccountKind, a: &str, b: &str) -> bool {
    match kind {
        AccountKind::InternetComputer => {
            match (LedgerAccount::from_str(a), LedgerAccount::from_str(b)) {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            }
        }
        AccountKind::Bitcoin(_) => match (Address::from_str(a), Address::from_str(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        },
        AccountKind::Evm => match (evm::parse_address(a), evm::parse_address(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        },
    }
}

/// Checks a contact action against the contacts and the wallet's own account
/// labels as they are now, and returns a `SetContact`'s addresses in
/// canonical form. Proposing checks it, and so does executing, since the
/// contacts and the labels may change in between.
pub fn check(
    action: &Action,
    contacts: &[(ContactId, StoredContact)],
    labels: &[String],
    network: BitcoinNetwork,
) -> Result<Vec<ContactAddress>, WalletError> {
    let invalid = |reason: String| Err(WalletError::InvalidArgument(reason));
    match action {
        Action::SetContact {
            id,
            name,
            addresses,
        } => {
            if let Some(id) = id
                && !contacts.iter().any(|(stored, _)| stored == id)
            {
                return Err(WalletError::NotFound(format!("contact {id}")));
            }
            if id.is_none() && contacts.len() >= MAX_CONTACTS {
                return Err(WalletError::PolicyViolation(format!(
                    "a wallet has at most {MAX_CONTACTS} contacts"
                )));
            }
            let trimmed = name.trim();
            if trimmed.is_empty() || name.len() > MAX_NAME_BYTES {
                return invalid(format!(
                    "a contact's name needs 1 to {MAX_NAME_BYTES} bytes"
                ));
            }
            if trimmed != name || name.chars().any(char::is_control) {
                return invalid(
                    "a contact's name has no control characters and no space at either end"
                        .to_string(),
                );
            }
            if !plain_name(name) {
                return invalid(
                    "a contact's name is written in plain English letters: A to Z, digits, spaces and ASCII punctuation"
                        .to_string(),
                );
            }
            if is_address(name) {
                return invalid("a contact's name cannot be an address".to_string());
            }
            let key = name_key(name);
            if let Some((other, _)) = contacts
                .iter()
                .find(|(other, contact)| Some(*other) != *id && name_key(&contact.name) == key)
            {
                return invalid(format!("contact {other} already has that name"));
            }
            if labels.iter().any(|label| name_key(label) == key) {
                return invalid("one of the wallet's accounts already has that name".to_string());
            }
            if addresses.is_empty() {
                return invalid("a contact needs at least one address".to_string());
            }
            let mut checked: Vec<ContactAddress> = Vec::new();
            for address in addresses {
                if checked.iter().any(|seen| seen.kind == address.kind) {
                    return invalid(format!(
                        "a contact has one address of each kind, and two of {:?}",
                        address.kind
                    ));
                }
                let canonical =
                    check_address(address.kind, &address.address, network).map_err(|reason| {
                        WalletError::InvalidArgument(format!(
                            "the {:?} address: {reason}",
                            address.kind
                        ))
                    })?;
                checked.push(ContactAddress {
                    kind: address.kind,
                    address: canonical,
                });
            }
            Ok(checked)
        }
        Action::RemoveContact(id) => {
            if !contacts.iter().any(|(stored, _)| stored == id) {
                return Err(WalletError::NotFound(format!("contact {id}")));
            }
            Ok(Vec::new())
        }
        _ => invalid("not a contact action".to_string()),
    }
}

/// Whether a proposer's transfer to `to`, from an account of `kind`, pays a
/// recipient the owners know (decision 0015): the address of one of the
/// wallet's own accounts, an address of a contact of that kind, or an
/// address the owner's request writes.
pub fn known_recipient(
    rule: ProposerRecipients,
    kind: AccountKind,
    to: &str,
    own: &[String],
    contacts: &[(ContactId, StoredContact)],
    request: Option<&str>,
) -> bool {
    if rule == ProposerRecipients::Any {
        return true;
    }
    own.iter().any(|address| same_address(kind, address, to))
        || contacts.iter().any(|(_, contact)| {
            contact
                .address(kind)
                .is_some_and(|address| same_address(kind, address, to))
        })
        || request.is_some_and(|request| words(request).any(|word| same_address(kind, word, to)))
}

/// The words of a request, with the punctuation around them taken off.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split_whitespace().map(|word| {
        word.trim_matches(|c: char| {
            matches!(
                c,
                ',' | '.'
                    | ';'
                    | ':'
                    | '!'
                    | '?'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '<'
                    | '>'
                    | '"'
                    | '\''
                    | '`'
            )
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCOUNT: &str = "rrkah-fqaaa-aaaaa-aaaaq-cai";
    const BTC: &str = "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq";
    const EVM: &str = "0x52908400098527886E0F7030069857D2E4169EE7";

    fn address(kind: AccountKind, address: &str) -> ContactAddress {
        ContactAddress {
            kind,
            address: address.to_string(),
        }
    }

    fn set(id: Option<ContactId>, name: &str, addresses: Vec<ContactAddress>) -> Action {
        Action::SetContact {
            id,
            name: name.to_string(),
            addresses,
        }
    }

    fn stored(name: &str, addresses: Vec<ContactAddress>) -> StoredContact {
        StoredContact {
            name: name.to_string(),
            addresses,
            added_at_ns: 0,
            added_by: Principal::anonymous(),
            changed_at_ns: None,
            changed_by: None,
            removed_at_ns: None,
        }
    }

    fn dana() -> Vec<(ContactId, StoredContact)> {
        vec![(
            0,
            stored(
                "Dana",
                vec![address(AccountKind::InternetComputer, ACCOUNT)],
            ),
        )]
    }

    fn check_new(
        name: &str,
        addresses: Vec<ContactAddress>,
    ) -> Result<Vec<ContactAddress>, WalletError> {
        check(
            &set(None, name, addresses),
            &dana(),
            &["Savings".to_string()],
            BitcoinNetwork::Mainnet,
        )
    }

    #[test]
    fn a_new_contact_needs_a_free_name_and_valid_addresses() {
        let ok = check_new(
            "Lee",
            vec![
                address(AccountKind::Bitcoin(BitcoinNetwork::Mainnet), BTC),
                address(AccountKind::Evm, &EVM.to_lowercase()),
            ],
        )
        .unwrap();
        // EVM addresses are kept as EIP-55 writes them.
        assert_eq!(ok[1].address, EVM);
        let refused = |name: &str, addresses: Vec<ContactAddress>| {
            matches!(
                check_new(name, addresses),
                Err(WalletError::InvalidArgument(_))
            )
        };
        let ic = || vec![address(AccountKind::InternetComputer, ACCOUNT)];
        assert!(refused("dana", ic()), "another contact's name, case aside");
        assert!(refused("  Dana", ic()), "spaces at an end");
        assert!(refused("savings", ic()), "an account's label");
        assert!(refused(ACCOUNT, ic()), "an address as a name");
        assert!(refused(BTC, ic()), "an address as a name");
        assert!(refused("", ic()));
        assert!(refused("Straße", ic()), "not plain English");
        assert!(refused("Ｄａｎａ", ic()), "full-width letters");
        assert!(refused("بهراد", ic()), "another script");
        assert!(refused("Dana\u{a0}Lee", ic()), "a no-break space");
        assert!(refused("Dana\tLee", ic()), "a tab");
        assert!(refused("1234", ic()), "no letter");
        assert!(check_new("Dana Lee-Smith (work)", ic()).is_ok());
        assert!(refused(&"x".repeat(MAX_NAME_BYTES + 1), ic()));
        assert!(refused("Lee", vec![]), "no address");
        assert!(refused("Lee", [ic(), ic()].concat()), "two of a kind");
        assert!(refused(
            "Lee",
            vec![address(AccountKind::InternetComputer, "not an account")]
        ));
        assert!(refused(
            "Lee",
            vec![address(AccountKind::Bitcoin(BitcoinNetwork::Testnet), BTC)]
        ));
        assert!(refused(
            "Lee",
            vec![address(AccountKind::Evm, &format!("0x{}", "0".repeat(40)))]
        ));
    }

    #[test]
    fn changing_a_contact_keeps_its_own_name_and_needs_it_to_exist() {
        let change = set(Some(0), "DANA", vec![address(AccountKind::Evm, EVM)]);
        assert!(check(&change, &dana(), &[], BitcoinNetwork::Mainnet).is_ok());
        let missing = set(Some(7), "Dana", vec![address(AccountKind::Evm, EVM)]);
        assert!(matches!(
            check(&missing, &dana(), &[], BitcoinNetwork::Mainnet),
            Err(WalletError::NotFound(_))
        ));
        assert!(
            check(
                &Action::RemoveContact(0),
                &dana(),
                &[],
                BitcoinNetwork::Mainnet
            )
            .is_ok()
        );
        assert!(matches!(
            check(
                &Action::RemoveContact(1),
                &dana(),
                &[],
                BitcoinNetwork::Mainnet
            ),
            Err(WalletError::NotFound(_))
        ));
    }

    #[test]
    fn a_proposer_pays_only_known_recipients_under_known() {
        let ic = AccountKind::InternetComputer;
        let own = vec!["ryjl3-tyaaa-aaaaa-aaaba-cai".to_string()];
        let known = |to: &str, request: Option<&str>| {
            known_recipient(ProposerRecipients::Known, ic, to, &own, &dana(), request)
        };
        assert!(known(ACCOUNT, None), "a contact");
        assert!(known("ryjl3-tyaaa-aaaaa-aaaba-cai", None), "an own account");
        let other = "qoctq-giaaa-aaaaa-aaaea-cai";
        assert!(!known(other, None));
        assert!(!known(other, Some("send 2 ICP to Dana")));
        assert!(
            known(other, Some(&format!("send 2 ICP to {other}."))),
            "written in the request"
        );
        assert!(known(other, Some(&format!("send 2 ICP to ({other})"))));
        assert!(!known(other, Some(&format!("send 2 ICP to {other}x"))));
        // Another kind's address of the contact is not its address of this kind.
        assert!(!known_recipient(
            ProposerRecipients::Known,
            AccountKind::Evm,
            EVM,
            &[],
            &dana(),
            None
        ));
        assert!(known_recipient(
            ProposerRecipients::Any,
            ic,
            other,
            &[],
            &[],
            None
        ));
    }

    #[test]
    fn names_compare_case_and_spaces_aside() {
        assert_eq!(name_key("  Dana   Lee "), "dana lee");
        assert_eq!(name_key("DANA\tLee"), "dana lee");
        // A label in compatibility forms is the same name as a contact's.
        assert_eq!(name_key("\u{ff33}avings"), name_key("Savings"));
        assert!(refused_label("\u{ff33}avings"));
    }

    #[test]
    fn an_account_cannot_take_a_contacts_name() {
        let dana = stored(
            "Dana",
            vec![address(AccountKind::InternetComputer, ACCOUNT)],
        );
        let contacts = vec![(1, dana)];
        assert!(check_label_free(" DANA ", &contacts).is_err());
        assert!(check_label_free("Dana's savings", &contacts).is_ok());
    }

    /// Whether a new contact named "Savings"-like `label` is refused beside an account labelled `label`.
    fn refused_label(label: &str) -> bool {
        let action = set(
            None,
            "Savings",
            vec![address(AccountKind::InternetComputer, ACCOUNT)],
        );
        matches!(
            check(&action, &[], &[label.to_string()], BitcoinNetwork::Mainnet),
            Err(WalletError::InvalidArgument(_))
        )
    }
}
