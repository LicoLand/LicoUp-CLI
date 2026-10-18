use super::model::DeviceTrustState;

pub(super) fn trusted_for_sensitive_use(value: &DeviceTrustState) -> bool {
    matches!(value, DeviceTrustState::Verified)
}

pub(super) fn usable_for_read_only(
    value: &DeviceTrustState,
    allow_unverified_read_only: bool,
) -> bool {
    match value {
        DeviceTrustState::Verified => true,
        DeviceTrustState::CrossSigned => allow_unverified_read_only,
        DeviceTrustState::Unverified => allow_unverified_read_only,
        DeviceTrustState::KeyChanged | DeviceTrustState::Revoked => false,
    }
}

pub(super) fn device_trust_decision_code(
    value: &DeviceTrustState,
    require_verified_device: bool,
    allow_unverified_read_only: bool,
) -> &'static str {
    match value {
        DeviceTrustState::Verified => "trusted",
        DeviceTrustState::CrossSigned => "cross_signature_requires_durable_epoch_validation",
        DeviceTrustState::Unverified if !require_verified_device && allow_unverified_read_only => {
            "read_only_unverified"
        }
        DeviceTrustState::Unverified => "verification_required",
        DeviceTrustState::KeyChanged => "identity_key_changed",
        DeviceTrustState::Revoked => "device_revoked",
    }
}
