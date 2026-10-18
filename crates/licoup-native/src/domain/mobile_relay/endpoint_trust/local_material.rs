mod accessors;
mod composition;
mod descriptor;
mod identity_generation;
mod material_mutation;
mod prekey_generation;
mod prekey_inventory;
mod protocol_reset;
mod rotation;
mod state;
mod state_codec;

pub(in crate::domain::mobile_relay) use composition::ensure_mobile_relay_endpoint_descriptor;
pub(in crate::domain::mobile_relay) use descriptor::local_endpoint_public_descriptor;
pub(in crate::domain::mobile_relay) use material_mutation::{
    ensure_mobile_relay_endpoint_material, existing_identity_requires_custody,
    local_identity_metadata_present, validate_existing_identity_custody,
};
pub(in crate::domain::mobile_relay) use protocol_reset::ensure_local_pairwise_protocol_compatible;
#[cfg(test)]
pub(in crate::domain::mobile_relay) use rotation::rotate_mobile_relay_local_identity_for_repair;
pub(in crate::domain::mobile_relay) use rotation::rotate_mobile_relay_one_time_prekeys;
pub(in crate::domain::mobile_relay) use state::LocalEndpointState;
pub(in crate::domain::mobile_relay) use state_codec::{
    hex_encode_bytes, local_endpoint_state, local_public_device_identity,
};

#[cfg(test)]
mod tests;
