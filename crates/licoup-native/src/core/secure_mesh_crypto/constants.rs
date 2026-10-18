use licoup_protocol_bindings::MAX_PADDING_BUCKET_BYTES;

pub const SECURE_MESH_CONTENT_CIPHER_SUITE: &str =
    "licomesh.secure-payload.v1.chacha20poly1305-hkdfsha256";
pub const SECURE_MESH_CONTENT_CRYPTO_STATUS: &str = "content_and_file_aead_available_authenticated_bucket_padding_available_pairwise_session_key_payload_codec_available_mls_exporter_diagnostic_only_product_group_messaging_disabled";

pub(super) const CONTENT_KEY_LEN: usize = 32;
pub(super) const CONTENT_NONCE_LEN: usize = 12;
pub(super) const AAD_HASH_LEN: usize = 32;
pub(super) const MAX_CONTENT_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_CONTEXT_FIELD_BYTES: usize = 4096;
pub(super) const MAX_CONTENT_TYPE_BYTES: usize = 255;
pub(super) const AEAD_TAG_LEN: usize = 16;
pub(super) const MAX_SEALED_CONTENT_BYTES: usize = MAX_PADDING_BUCKET_BYTES;
pub(super) const AAD_MAGIC: &[u8] = b"LCOSM-AAD-v1";
pub(super) const PLAINTEXT_MAGIC: &[u8] = b"LCOSM-PT-v1";
pub(super) const PADDED_PLAINTEXT_MAGIC: &[u8] = b"LCOSM-PAD-v1";
pub(super) const HEADER_MAGIC: &[u8] = b"LCOSM-HDR-v1";
pub(super) const ADDITIONAL_AAD_MAGIC: &[u8] = b"LCOSM-ADDITIONAL-AAD-v1";
pub(super) const HKDF_SALT_DOMAIN: &[u8] = b"licomesh.secure-mesh.payload-aead.hkdf-salt.v1";
pub(super) const HKDF_INFO_DOMAIN: &[u8] = b"licomesh.secure-mesh.payload-aead.hkdf-info.v1";
pub(super) const MAX_ADDITIONAL_AAD_BYTES: usize = 16 * 1024;
pub(super) const PRIVATE_CONTEXT_AEAD_AAD: &[u8] =
    b"licomesh.secure-mesh.private-context-aead.public-profile.v2";
pub(super) const PRIVATE_CONTEXT_FRAME_MAGIC: &[u8] = b"LCOSM-PRIVATE-CONTEXT-FRAME-v2";
pub(super) const PRIVATE_CONTEXT_HEADER_MAGIC: &[u8] = b"LCOSM-PRIVATE-CONTEXT-HEADER-v2";
pub(super) const PRIVATE_CONTEXT_HKDF_SALT_DOMAIN: &[u8] =
    b"licomesh.secure-mesh.private-context-aead.hkdf-salt.v2";
pub(super) const PRIVATE_CONTEXT_HKDF_INFO_DOMAIN: &[u8] =
    b"licomesh.secure-mesh.private-context-aead.hkdf-info.v2";
