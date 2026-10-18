mod authority;
mod authorization;
mod claim;
mod clock;
mod config;
mod ensure;
mod freshness;
#[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
mod test_support;
mod verifier;

pub(in crate::domain::mobile_relay) use authority::open_mobile_relay_directory_authority;
pub(in crate::domain::mobile_relay) use authorization::{
    authorize_exact_local_directory_response, authorize_local_pairwise_directory,
    authorize_peer_pairwise_directory, authorize_peer_pairwise_directory_for_purpose,
};
pub(in crate::domain::mobile_relay) use claim::build_local_directory_claim;
pub(in crate::domain::mobile_relay) use clock::current_secure_mesh_kt_gate_epoch_seconds;
#[cfg(test)]
pub(in crate::domain::mobile_relay) use clock::set_kt_freshness_now_override;
pub(in crate::domain::mobile_relay) use config::{
    configured_directory_scope_commitment, configured_kt_pin, derive_local_publication_purpose,
    parse_local_directory_authorization_purpose, validate_canonical_sha256_hex,
};
pub(in crate::domain::mobile_relay) use ensure::ensure_mobile_relay_key_transparency;
pub(in crate::domain::mobile_relay) use freshness::require_current_pairwise_directory_authority;
#[cfg(test)]
pub(in crate::domain::mobile_relay) use test_support::{
    with_mobile_relay_test_kt_authority_scope, with_mobile_relay_test_kt_log,
};

#[cfg(test)]
mod tests;
