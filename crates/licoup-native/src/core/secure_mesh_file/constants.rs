pub const SECURE_MESH_FILE_MANIFEST_CONTENT_TYPE: &str =
    "application/licomesh.secure-mesh.file.v1+json";
pub const SECURE_MESH_FILE_CHUNK_CONTENT_TYPE: &str =
    "application/licomesh.secure-mesh.file.v1+json";
pub const SECURE_MESH_FILE_KEY_ENVELOPE_CONTENT_TYPE: &str =
    "application/licomesh.secure-mesh.file-key-envelope.v2+json";
pub const SECURE_MESH_FILE_CRYPTO_STATUS: &str = "file_root_key_domain_separation_pairwise_device_mls_epoch_wrap_manifest_chunk_receipt_available";
pub const SECURE_MESH_FILE_KEY_SUITE: &str =
    "licomesh.secure-mesh.file-key.v2.xchacha20poly1305-hkdfsha256";
pub(super) const FILE_MANIFEST_MAGIC: &[u8] = b"LCOSM-FM-v1";
pub(super) const FILE_CHUNK_MAGIC: &[u8] = b"LCOSM-FC-v1";
pub(super) const FILE_ROOT_KEY_BYTES: usize = 32;
pub(super) const FILE_KEY_WRAP_SECRET_BYTES: usize = 32;
pub(super) const FILE_KEY_ENVELOPE_NONCE_BYTES: usize = 24;
pub(super) const FILE_KEY_ENVELOPE_TAG_BYTES: usize = 16;
pub(super) const FILE_KEY_ENVELOPE_SCHEMA: &str = "licomesh.secure-mesh.file-key-envelope.v2";
pub(super) const FILE_KEY_ENVELOPE_FRAME_MAGIC: &[u8] =
    b"LCOSM-FILE-KEY-ENVELOPE-XCHACHA20POLY1305-v2";
pub(super) const FILE_KEY_ENVELOPE_MAX_JSON_BYTES: usize = 4 * 1024;
pub(super) const FILE_AAD_MAGIC: &[u8] = b"LCOSM-FILE-AAD-v2";
pub(super) const FILE_HKDF_SALT: &[u8] = b"licomesh.secure-mesh.file.hkdf-salt.v2";
pub(super) const FILE_HKDF_MANIFEST_DOMAIN: &[u8] = b"licomesh.secure-mesh.file.manifest-key.v2";
pub(super) const FILE_HKDF_CHUNK_DOMAIN: &[u8] = b"licomesh.secure-mesh.file.chunk-key.v2";
pub(super) const FILE_HKDF_CHUNK_HASH_DOMAIN: &[u8] =
    b"licomesh.secure-mesh.file.chunk-hash-key.v2";
pub(super) const FILE_HKDF_RECEIPT_DOMAIN: &[u8] = b"licomesh.secure-mesh.file.receipt-key.v2";
pub(super) const FILE_HKDF_KEY_WRAP_DOMAIN: &[u8] = b"licomesh.secure-mesh.file.key-wrap-key.v2";
pub(super) const FILE_AAD_MANIFEST_PURPOSE: &[u8] = b"manifest";
pub(super) const FILE_AAD_CHUNK_PURPOSE: &[u8] = b"chunk";
pub(super) const FILE_AAD_CHUNK_HASH_PURPOSE: &[u8] = b"chunk-hash";
pub(super) const FILE_AAD_RECEIPT_PURPOSE: &[u8] = b"receipt";
pub(super) const FILE_AAD_KEY_WRAP_PURPOSE: &[u8] = b"key-wrap";
pub(super) const MAX_FILE_CRYPTO_CONTEXT_BYTES: usize = 4096;
pub(super) const MAX_FILE_NAME_BYTES: usize = 255;
pub(super) const MAX_MIME_BYTES: usize = 255;
pub(super) const MAX_RELATIVE_PATH_BYTES: usize = 4096;
pub(super) const MAX_CHUNK_BYTES: usize = 8 * 1024 * 1024;
pub(super) const MAX_CHUNK_COUNT: u32 = 100_000;
pub(super) const DEFAULT_MAX_QUEUED_FILE_TRANSFERS: usize = 32;
pub(super) const DEFAULT_MAX_QUEUED_FILE_CIPHERTEXT_BYTES: usize = 512 * 1024 * 1024;
pub(super) const DEFAULT_FILE_CONFLICT_POLICY: &str = "fail_if_exists";
