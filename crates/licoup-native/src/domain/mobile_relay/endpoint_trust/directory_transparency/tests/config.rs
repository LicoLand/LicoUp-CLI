use super::super::config::{
    parse_local_directory_authorization_purpose, validate_canonical_sha256_hex,
};

#[test]
fn directory_purpose_and_scope_digest_are_strictly_canonical() {
    assert_eq!(
        parse_local_directory_authorization_purpose("self-monitor")
            .unwrap()
            .stable_code(),
        "self-monitor"
    );
    assert!(parse_local_directory_authorization_purpose("pairing").is_err());
    assert!(validate_canonical_sha256_hex(&"a".repeat(64), "scope").is_ok());
    assert!(validate_canonical_sha256_hex(&"A".repeat(64), "scope").is_err());
}
