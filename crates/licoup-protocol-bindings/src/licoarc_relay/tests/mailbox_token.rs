//! Mailbox identifier validation checks.

use super::support::*;

#[test]
fn mailbox_token_parser_enforces_canonical_fixed_size_base64url() {
    let canonical = general_purpose::URL_SAFE_NO_PAD.encode([0x41; MAILBOX_TOKEN_BYTES]);
    let token = SecureMeshMailboxToken::from_base64url(&canonical).unwrap();
    assert_eq!(token.as_str(), canonical);
    assert!(!format!("{token:?}").contains(&canonical));

    assert!(SecureMeshMailboxToken::from_base64url(format!("{canonical}=")).is_err());
    assert!(
        SecureMeshMailboxToken::from_base64url(
            general_purpose::URL_SAFE_NO_PAD.encode([0x41; MAILBOX_TOKEN_BYTES - 1]),
        )
        .is_err()
    );
}
