//! Lico Arc carrier labels and allocation bounds.

pub const LICOARC_RELAY_CONTRACT_VERSION: &str = "licoarc.relay.v1";
pub const LICOARC_RELAY_OUTER_FIELDS: [&str; 5] = [
    "contractVersion",
    "envelopeId",
    "mailboxId",
    "ciphertext",
    "expiresAt",
];
pub const LICOARC_MAX_CIPHERTEXT_CHARS: usize = 1_048_576;
pub const LICOARC_ENCRYPTED_HEADER_BYTES: usize = 4 * 1024;
pub const SECURE_MESH_MAILBOX_ROTATION_WINDOW_SECONDS: u64 = 15 * 60;
pub const SECURE_MESH_MAILBOX_PREVIOUS_WINDOW_COUNT: usize = 1;

pub(super) const DELIVERY_SECRET_BYTES: usize = 32;
pub(super) const CHANNEL_BINDING_BYTES: usize = 32;
pub(super) const DELIVERY_ID_BYTES: usize = 24;
pub(super) const MAILBOX_TOKEN_BYTES: usize = 32;
pub(super) const LICOARC_ID_MIN_CHARS: usize = 16;
pub(super) const LICOARC_ID_MAX_CHARS: usize = 128;
pub(super) const LICOARC_EXPIRES_AT_MAX_CHARS: usize = 64;
pub(super) const RELAY_HEADER_KEY_BYTES: usize = 32;
pub(super) const RELAY_HEADER_NONCE_BYTES: usize = 24;
pub(super) const RELAY_HEADER_TAG_BYTES: usize = 16;
pub(super) const RELAY_HEADER_FRAME_BYTES: usize =
    LICOARC_ENCRYPTED_HEADER_BYTES - RELAY_HEADER_NONCE_BYTES - RELAY_HEADER_TAG_BYTES;
pub(super) const RELAY_HEADER_FRAME_MAGIC: &[u8] =
    b"LICOUP-LICOARC-PRIVATE-HEADER-XCHACHA20POLY1305-v1";
pub(super) const RELAY_HEADER_LENGTH_BYTES: usize = 4;
pub(super) const MAX_RELAY_PRIVATE_HEADER_BYTES: usize =
    RELAY_HEADER_FRAME_BYTES - RELAY_HEADER_FRAME_MAGIC.len() - RELAY_HEADER_LENGTH_BYTES;
pub(super) const JSON_SAFE_INTEGER_MAX: u64 = 9_007_199_254_740_991;
pub(super) const MAX_RELAY_ENVELOPE_JSON_BYTES: usize =
    LICOARC_MAX_CIPHERTEXT_CHARS + (2 * LICOARC_ID_MAX_CHARS) + 1_024;
pub(super) const MAILBOX_HKDF_SALT: &[u8] = b"licoup.licoarc.mailbox.hkdf-salt.v1";
pub(super) const MAILBOX_HKDF_INFO: &[u8] = b"licoup.licoarc.mailbox.hkdf-info.v1";
pub(super) const OUTER_AAD_MAGIC: &[u8] = b"LICOUP-LICOARC-RELAY-OUTER-AAD-v1";
pub(super) const CARRIER_MAGIC: &[u8; 4] = b"LARC";
pub(super) const CARRIER_VERSION: u8 = 1;
pub(super) const CARRIER_LENGTH_BYTES: usize = 4;
pub(super) const CARRIER_PREFIX_BYTES: usize = CARRIER_MAGIC.len() + 1 + (2 * CARRIER_LENGTH_BYTES);
