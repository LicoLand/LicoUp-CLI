//! Synthetic fixtures shared by the carrier tests.

pub(super) use super::super::carrier::{decode_carrier, encode_carrier, preflight_carrier_size};
pub(super) use super::super::constants::*;
pub(super) use super::super::private_header::{
    open_private_relay_header, seal_private_relay_header, seal_private_relay_header_with_nonce,
};
pub(super) use super::super::private_header_frame::{
    decode_private_relay_header_frame, encode_private_relay_header_frame,
};
pub(super) use super::super::{
    LicoArcRelayEnvelope, LicoArcRelayEnvelopeDraft, SecureMeshDeliverySecret,
    SecureMeshMailboxDirection, SecureMeshMailboxSchedule, SecureMeshMailboxToken,
    SecureMeshRelayChannelBinding,
};
pub(super) use crate::{
    LARGE_PADDING_BUCKET_STEP_BYTES, MAX_PADDING_BUCKET_BYTES, MIN_PADDING_BUCKET_BYTES,
    POWER_OF_TWO_PADDING_LIMIT_BYTES,
};
pub(super) use base64::{Engine as _, engine::general_purpose};
pub(super) use serde_json::{Value, json};

pub(super) const VECTOR_TIME_SECONDS: u64 = 1_800_000_123;
pub(super) const FIXTURE_EXPIRY: &str = "2030-01-01T00:00:00Z";

pub(super) fn schedule(direction: SecureMeshMailboxDirection) -> SecureMeshMailboxSchedule {
    SecureMeshMailboxSchedule::new(
        SecureMeshDeliverySecret::from_bytes([0x11; DELIVERY_SECRET_BYTES]),
        direction,
        SecureMeshRelayChannelBinding::from_bytes([0x22; CHANNEL_BINDING_BYTES]),
    )
}

pub(super) fn envelope_fixture() -> LicoArcRelayEnvelope {
    let mailbox = schedule(SecureMeshMailboxDirection::PairwiseInitiatorToResponder)
        .token_for_unix_seconds(VECTOR_TIME_SECONDS)
        .unwrap();
    LicoArcRelayEnvelope::new_with_envelope_id(
        &mailbox,
        FIXTURE_EXPIRY,
        &[0x33; LICOARC_ENCRYPTED_HEADER_BYTES],
        &[0x44; MIN_PADDING_BUCKET_BYTES],
        [0x55; DELIVERY_ID_BYTES],
    )
    .unwrap()
}
