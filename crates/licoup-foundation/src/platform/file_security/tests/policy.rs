#[test]
fn private_file_limits_are_explicit_and_independent() {
    assert_eq!(
        super::super::policy::PRIVATE_STATE_FILE_MAX_BYTES,
        64 * 1024
    );
    assert_eq!(
        super::super::policy::PRIVATE_APPEND_FILE_MAX_BYTES,
        64 * 1024 * 1024
    );
    assert_eq!(
        super::super::policy::PRIVATE_APPEND_LINE_MAX_BYTES,
        4 * 1024 * 1024
    );
    assert_eq!(
        super::super::policy::PRIVATE_LOCK_MARKER,
        b"licomesh-private-lock-v1"
    );
}
