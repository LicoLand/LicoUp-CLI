use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::super::validation::{
    ensure_active_trust_state, ensure_not_expired, ensure_signature_shape,
};
use crate::core::secure_mesh_trust::DeviceTrustState;

fn timestamp(value: &str) -> OffsetDateTime {
    OffsetDateTime::parse(value, &Rfc3339).unwrap()
}

#[test]
fn trust_validation_is_fail_closed_except_explicit_unverified_policy() {
    assert!(ensure_active_trust_state(DeviceTrustState::Verified, true).is_ok());
    assert!(ensure_active_trust_state(DeviceTrustState::Unverified, false).is_ok());
    for state in [
        DeviceTrustState::Unverified,
        DeviceTrustState::CrossSigned,
        DeviceTrustState::KeyChanged,
        DeviceTrustState::Revoked,
    ] {
        assert!(ensure_active_trust_state(state, true).is_err());
    }
}

#[test]
fn created_at_accepts_the_exact_clock_skew_boundary_only() {
    let now = timestamp("2026-01-01T00:00:00Z");
    assert!(
        ensure_not_expired(
            "2026-01-01T00:05:00Z",
            "2026-01-02T00:00:00Z",
            now,
            "vector",
        )
        .is_ok()
    );
    assert!(
        ensure_not_expired(
            "2026-01-01T00:05:01Z",
            "2026-01-02T00:00:00Z",
            now,
            "vector",
        )
        .is_err()
    );
}

#[test]
fn expiry_order_and_signature_encoding_are_bounded() {
    let now = timestamp("2026-01-01T00:00:00Z");
    assert!(
        ensure_not_expired(
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00Z",
            now,
            "vector",
        )
        .is_err()
    );
    assert!(ensure_signature_shape("", "vector").is_err());
    assert!(ensure_signature_shape(&"a".repeat(257), "vector").is_err());
    assert!(ensure_signature_shape("valid_shape", "vector").is_ok());
}
