//! LicoUp's endpoint-owned codec for the neutral Lico Arc relay envelope.
//!
//! The public model is exactly the five-field Lico Arc v1 contract. LicoUp's encrypted private
//! header and authenticated padded content are carried inside one versioned, canonical base64url
//! value. Stations remain unaware of and unauthoritative over the carrier contents.

mod aad;
mod carrier;
mod codec;
mod constant_time;
mod constants;
mod delivery;
mod draft;
mod envelope;
mod mailbox;
mod private_header;
mod private_header_frame;
mod wire;

pub use constants::{
    LICOARC_ENCRYPTED_HEADER_BYTES, LICOARC_MAX_CIPHERTEXT_CHARS, LICOARC_RELAY_CONTRACT_VERSION,
    LICOARC_RELAY_OUTER_FIELDS, SECURE_MESH_MAILBOX_PREVIOUS_WINDOW_COUNT,
    SECURE_MESH_MAILBOX_ROTATION_WINDOW_SECONDS,
};
pub use delivery::{SecureMeshDeliverySecret, SecureMeshRelayChannelBinding};
pub use draft::LicoArcRelayEnvelopeDraft;
pub use envelope::LicoArcRelayEnvelope;
pub use mailbox::{SecureMeshMailboxDirection, SecureMeshMailboxSchedule, SecureMeshMailboxToken};
pub use private_header::{open_private_relay_header, seal_private_relay_header};

#[cfg(test)]
mod tests;
