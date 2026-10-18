mod authority;
mod config;
mod contract;
mod dispatcher;
mod gossip;
mod persistence;
mod projection;
mod provision;
mod publication;
mod revocation;
mod self_monitor;
mod status;

#[cfg(test)]
pub(in crate::domain::mobile_relay) const SECURE_MESH_KT_GOSSIP_CONTROL_TYPE: &str =
    contract::SECURE_MESH_KT_GOSSIP_CONTROL_TYPE;
pub use contract::SECURE_MESH_KT_NATIVE_ACTIONS;
pub use dispatcher::dispatch_key_transparency_action;

#[cfg(test)]
pub(in crate::domain::mobile_relay) use authority::{
    authority_configuration_matches, key_transparency_configure_authority,
    parse_kt_authority_proposal, read_kt_authority_challenge,
};

#[cfg(test)]
mod tests;
