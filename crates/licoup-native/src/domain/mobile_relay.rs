mod command_sync;
mod config;
mod endpoint_collaboration_gate;
mod endpoint_ports;
pub mod endpoint_transport;
mod endpoint_trust;
mod key_transparency;
mod pairing;
mod pairwise_session;
mod relay_operations;
mod secret_custody;
mod support;

pub use command_sync::commands_sync;
pub use config::{config_get, config_set};
pub(crate) use config::{migrate_config_document, validate_current_config_document};
/// The caller-owned port spine the fixed SDK endpoint is composed from: the
/// custody, durable-state, clock, and carrier adapters, the admitted-only
/// composition point, the SDK's own verified-fact consumer, and the device
/// replacement composition that binds SDK-verified authority to the caller's
/// subject, source device, new identity and operation.
pub use endpoint_ports::{
    AdmittedProtocolWork, AtomicStateAdapter, ClockAdapter, CustodyHandles, KeyCustodyAdapter,
    PacketCarrierAdapter, ProtocolAdmissionRefusal, ProtocolPorts, ProtocolWorkSetup,
    ReplacementRequest, admit_replacement, admit_replacement_with_authority,
};
pub use key_transparency::{SECURE_MESH_KT_NATIVE_ACTIONS, dispatch_key_transparency_action};
pub use licoup_protocol_bindings::EndpointConsumer;
pub use pairing::{pairing_claim, pairing_create, pairing_revoke, pairing_status};
pub use relay_operations::{
    command_create, command_create_secure, command_result, command_result_replay_proof,
    command_result_secure, commands_poll, e2ee_status, pc_check_in,
};
pub use secret_custody::{
    e2ee_secret_store_cleanup, e2ee_secret_store_cleanup_inventory, e2ee_secret_store_self_test,
    selected_mobile_relay_capability_evaluation, with_mobile_relay_secret_store_override,
    with_pairwise_secret_store_override,
};

#[cfg(test)]
pub(crate) use endpoint_trust::{
    initialize_secure_mesh_mls_test_endpoint, initialize_secure_mesh_mls_test_peer,
    refresh_secure_mesh_mls_test_directory_authority, secure_mesh_mls_test_directory_response,
};
pub(crate) use endpoint_trust::{
    persisted_mobile_relay_peer_trust_state, secure_mesh_kt_authority_path,
    secure_mesh_mls_public_directory_context, secure_mesh_mls_state_dir,
};
#[cfg(test)]
pub(crate) use secret_custody::test_runtime_secret_material;
pub(crate) use secret_custody::{
    ensure_secure_mesh_protected_operation_allowed, prepare_recovered_custody_metadata,
    with_secure_mesh_mls_participant,
};

#[cfg(test)]
mod tests;
