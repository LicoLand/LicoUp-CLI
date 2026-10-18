mod allow_list;
mod command_handlers;
mod delivery;
mod envelope;
mod mailbox;
mod station;
mod status;

pub use command_handlers::{
    command_create, command_create_secure, command_result, command_result_replay_proof,
    command_result_secure, commands_poll, pc_check_in,
};
pub use status::e2ee_status;

pub(super) use allow_list::allowed_agent_ids;
pub(super) use command_handlers::{
    pc_check_in_with_context, receive_station_envelopes_with_config,
};
pub(super) use delivery::local_command_from_relay_delivery;
pub(super) use envelope::{
    relay_envelope_from_value, secure_envelope_param, validate_secure_envelope,
};
#[cfg(test)]
pub(super) use mailbox::local_canonical_mailbox_tokens_at_epoch;
pub(super) use mailbox::{canonical_mailbox_token, current_mailbox_rotation_epoch};
pub(super) use station::{
    deletion_transport_hint, delivery_transport_hint, lease_transport_hint, station_binding_digest,
    station_context, station_lease_seconds,
};

#[cfg(test)]
mod tests;
