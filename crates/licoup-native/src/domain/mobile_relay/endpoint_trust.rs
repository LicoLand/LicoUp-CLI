#[cfg(test)]
use super::config::default_config;
use super::{pairwise_session::*, relay_operations::*, support::*};

mod directory_transparency;
mod local_material;
mod pairing_presentation;
mod pairwise_codec;
mod peer_trust;
mod persistence;
mod primitives;

#[cfg(test)]
mod tests;

pub(in crate::domain::mobile_relay) use directory_transparency::{
    authorize_exact_local_directory_response, authorize_local_pairwise_directory,
    authorize_peer_pairwise_directory, authorize_peer_pairwise_directory_for_purpose,
    build_local_directory_claim, configured_directory_scope_commitment, configured_kt_pin,
    current_secure_mesh_kt_gate_epoch_seconds, derive_local_publication_purpose,
    ensure_mobile_relay_key_transparency, open_mobile_relay_directory_authority,
    parse_local_directory_authorization_purpose, require_current_pairwise_directory_authority,
    validate_canonical_sha256_hex,
};
#[cfg(test)]
pub(in crate::domain::mobile_relay) use directory_transparency::{
    set_kt_freshness_now_override, with_mobile_relay_test_kt_authority_scope,
    with_mobile_relay_test_kt_log,
};
#[cfg(test)]
pub(in crate::domain::mobile_relay) use local_material::rotate_mobile_relay_local_identity_for_repair;
pub(in crate::domain::mobile_relay) use local_material::{
    LocalEndpointState, ensure_local_pairwise_protocol_compatible,
    ensure_mobile_relay_endpoint_descriptor, ensure_mobile_relay_endpoint_material,
    existing_identity_requires_custody, hex_encode_bytes, local_endpoint_public_descriptor,
    local_endpoint_state, local_identity_metadata_present, local_public_device_identity,
    rotate_mobile_relay_one_time_prekeys, validate_existing_identity_custody,
};
pub(in crate::domain::mobile_relay) use pairing_presentation::*;
pub(in crate::domain::mobile_relay) use pairwise_codec::*;
pub(in crate::domain::mobile_relay) use peer_trust::*;
pub(in crate::domain::mobile_relay) use primitives::*;

pub(crate) use persistence::{
    persisted_mobile_relay_peer_trust_state, secure_mesh_kt_authority_path,
    secure_mesh_mls_public_directory_context, secure_mesh_mls_state_dir,
};

#[cfg(test)]
pub(crate) use tests::{
    initialize_secure_mesh_mls_test_endpoint, initialize_secure_mesh_mls_test_peer,
    refresh_secure_mesh_mls_test_directory_authority, secure_mesh_mls_test_directory_response,
};
