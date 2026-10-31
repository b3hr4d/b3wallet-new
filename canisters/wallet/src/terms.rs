//! Terms of a request (decision 0014). An agent writes the terms it reads in an owner's request, and an owner
//! confirms them on the wallet's own page, or says no, before a transfer may answer the request. The wallet keeps
//! that order and the record of who said what. Reading the terms, writing them in words and checking a transfer
//! against them is `@meantjs/consent/terms` on the page; the wallet stores what the page confirmed, and checks only
//! that the record holds together: its id is the SHA-256 of the terms it names.

use crate::{ConfirmTermsArgs, TermsDraft, WalletError};
use sha2::{Digest, Sha256};

/// The longest field of a draft, in bytes of UTF-8: an ICRC-1 account with its subaccount fits.
pub const MAX_FIELD_BYTES: usize = 160;
/// The longest confirmed terms, as JSON, in bytes.
pub const MAX_TERMS_BYTES: usize = 2_048;
/// The longest text an owner confirms, in bytes.
pub const MAX_TEXT_BYTES: usize = 600;
/// The most drafts a request takes. After that the owner writes a new request.
pub const MAX_DRAFTS: u32 = 3;

/// The draft with each field trimmed, or why it is refused.
pub fn check_draft(draft: TermsDraft) -> Result<TermsDraft, WalletError> {
    let field = |name: &str, value: String| {
        let value = value.trim().to_string();
        if value.is_empty() || value.len() > MAX_FIELD_BYTES || value.chars().any(char::is_control)
        {
            return Err(WalletError::InvalidArgument(format!(
                "{name} is 1 to {MAX_FIELD_BYTES} bytes, with no control characters"
            )));
        }
        Ok(value)
    };
    Ok(TermsDraft {
        token: field("token", draft.token)?,
        to: field("to", draft.to)?,
        max_amount: field("max_amount", draft.max_amount)?,
        expires_by: field("expires_by", draft.expires_by)?,
    })
}

/// Refuses a confirmation whose parts do not hold together: terms and text of a sensible size, and an id that is
/// the SHA-256 of the terms, in lowercase hex.
pub fn check_confirmation(args: &ConfirmTermsArgs) -> Result<(), WalletError> {
    if args.terms.is_empty() || args.terms.len() > MAX_TERMS_BYTES {
        return Err(WalletError::InvalidArgument(format!(
            "terms are 1 to {MAX_TERMS_BYTES} bytes of JSON"
        )));
    }
    if args.text.trim().is_empty() || args.text.len() > MAX_TEXT_BYTES {
        return Err(WalletError::InvalidArgument(format!(
            "the confirmed text is 1 to {MAX_TEXT_BYTES} bytes"
        )));
    }
    if args.id != hex(&Sha256::digest(args.terms.as_bytes())) {
        return Err(WalletError::InvalidArgument(
            "id is not the SHA-256 of the terms, in lowercase hex".to_string(),
        ));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> TermsDraft {
        TermsDraft {
            token: " ICP ".to_string(),
            to: "rrkah-fqaaa-aaaaa-aaaaq-cai".to_string(),
            max_amount: "1.5".to_string(),
            expires_by: "2026-11-30".to_string(),
        }
    }

    fn confirmation(terms: &str) -> ConfirmTermsArgs {
        ConfirmTermsArgs {
            version: 1,
            terms: terms.to_string(),
            id: hex(&Sha256::digest(terms.as_bytes())),
            text: "rrkah-fqaaa-aaaaa-aaaaq-cai may be sent at most 1.5 ICP, until the end of 30 November 2026 (UTC)."
                .to_string(),
        }
    }

    #[test]
    fn a_draft_is_trimmed_and_each_field_must_be_there() {
        assert_eq!(check_draft(draft()).unwrap().token, "ICP");
        for change in [
            |d: &mut TermsDraft| d.token = "  ".to_string(),
            |d: &mut TermsDraft| d.to = "x".repeat(MAX_FIELD_BYTES + 1),
            |d: &mut TermsDraft| d.max_amount = "1\n5".to_string(),
            |d: &mut TermsDraft| d.expires_by = String::new(),
        ] {
            let mut d = draft();
            change(&mut d);
            assert!(matches!(
                check_draft(d),
                Err(WalletError::InvalidArgument(_))
            ));
        }
        let mut longest = draft();
        longest.to = "x".repeat(MAX_FIELD_BYTES);
        assert!(check_draft(longest).is_ok());
    }

    /// The id the library gives terms is the SHA-256 of their JSON: the wallet takes a confirmation only when the
    /// two agree, so a record never names terms it does not hold.
    #[test]
    fn a_confirmation_names_the_terms_it_holds() {
        let terms = r#"{"amount":{"unlimited":false,"units":"150000000"}}"#;
        assert_eq!(check_confirmation(&confirmation(terms)), Ok(()));
        // The vector of FIPS 180-2: SHA-256("abc").
        assert_eq!(
            confirmation("abc").id,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // What the wallet's page sends for terms that name an account by its label (ui/test/terms.test.ts): the
        // library hashes the JSON's UTF-8, and so does the wallet, "É" included.
        let page = ConfirmTermsArgs {
            terms: concat!(
                r#"{"amount":{"units":"150000000","unlimited":false},"decimals":8,"#,
                r#""ends":{"at":"1796079599.999999999","day":"2026-11-30"},"name":"Épargne","#,
                r#""symbol":"ICP","to":"uxrrr-q7777-77774-qaaaq-cai-eiw7ojy.1","#,
                r#""token":"ryjl3-tyaaa-aaaaa-aaaba-cai","utcOffsetMinutes":60,"#,
                r#""written":{"expires_by":"2026-11-30","max_amount":"1.5","to":"Épargne","token":"ICP"}}"#,
            )
            .to_string(),
            id: "4b53ee47f52b083bdecb7180cbba52a39a1dd840a8e7e4d0649fe73600ae4325".to_string(),
            ..confirmation(terms)
        };
        assert_eq!(check_confirmation(&page), Ok(()));
        let mut other = confirmation(terms);
        other.terms.push(' ');
        assert!(matches!(
            check_confirmation(&other),
            Err(WalletError::InvalidArgument(_))
        ));
        let mut upper = confirmation(terms);
        upper.id = upper.id.to_uppercase();
        assert!(matches!(
            check_confirmation(&upper),
            Err(WalletError::InvalidArgument(_))
        ));
        for (terms, text) in [
            ("", "x"),
            (&*"x".repeat(MAX_TERMS_BYTES + 1), "x"),
            ("{}", " "),
            ("{}", &*"x".repeat(MAX_TEXT_BYTES + 1)),
        ] {
            let mut args = confirmation(terms);
            args.text = text.to_string();
            assert!(
                matches!(
                    check_confirmation(&args),
                    Err(WalletError::InvalidArgument(_))
                ),
                "{terms:.20} {text:.20}"
            );
        }
    }
}
