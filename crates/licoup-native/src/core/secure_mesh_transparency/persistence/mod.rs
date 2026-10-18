//! Durable transparency state, split by independent persistence responsibility.

mod checkpoint;
mod directory;
mod gossip;
mod schema;
mod sql;
mod time_guard;

pub(super) use checkpoint::{
    CheckpointTransition, advance_checkpoint_transaction, latest_checkpoint_connection,
};
#[cfg(test)]
pub(super) use directory::{
    enforce_directory_authorization_quota, enforce_directory_label_quota,
    reclaim_stale_directory_authorizations,
};
pub(super) use directory::{
    enforce_directory_latest_transaction, persist_directory_authorization_transaction,
};
pub(super) use gossip::{
    persist_gossip_observation_transaction, require_fresh_gossip_checkpoint_transaction,
    require_fresh_gossip_observation_transaction,
};
pub use schema::reset_kt_persistent_authority_state;
pub(super) use schema::{initialize_kt_schema, initialize_or_validate_pin};
pub(super) use sql::sql_to_u64;
#[cfg(test)]
pub(super) use sql::u64_to_sql;
pub(super) use time_guard::{
    advance_durable_time_watermark, authenticated_sth_temporal_block_reason,
    persist_security_block, persist_security_block_connection,
    verify_authenticated_sth_freshness_or_block,
};
