pub const SECURE_MESH_MLS_CIPHER_SUITE: &str =
    "MLS_128_DHKEMX25519_CHACHA20POLY1305_SHA256_Ed25519+ML_KEM_1024_EPOCH_PAYLOAD_HYBRID";
pub const SECURE_MESH_GROUP_MLS_PROTOCOL_VERSION: &str =
    "licomesh.secure-mesh.group-mls.mlkem1024-epoch-payload-hybrid.v1";
pub const SECURE_MESH_MLS_STATUS: &str = "openmls_classical_control_plane_mlkem1024_epoch_hybrid_payload_selected_custody_durable_group_state_identity_bound_capability_negotiated";

pub(super) const MLS_PAYLOAD_EXPORT_LABEL: &str = "licomesh.secure-mesh.mls.payload-content-key.v2";
pub(super) const MLS_PAYLOAD_EXPORT_CONTEXT_MAGIC: &[u8] = b"LCOSM-MLS-PAYLOAD-EXPORT-v2";
pub(crate) const SECURE_MESH_MLS_APPLICATION_PUBLIC_AAD: &[u8] =
    b"licomesh.secure-mesh.mls.application.public-domain-profile.v2";
pub(super) const MLS_PRIVATE_CONTEXT_PAYLOAD_MAGIC: &[u8] = b"LCOSM-MLS-PRIVATE-CONTEXT-PAYLOAD-v2";
pub(super) const MLS_PAYLOAD_CONTENT_KEY_LEN: usize = 32;
pub(super) const MLS_PROVIDER_SECRET_SCHEMA_VERSION: u32 = 2;
pub(super) const MLS_KEY_PACKAGE_MAGIC: &[u8] = b"LCOSM-MLS-KEYPACKAGE-MLKEM1024-v1";
pub(super) const MLS_EPOCH_SECRET_STORE_CLASS: &str = "mlsEpochSecret";
pub(super) const MLS_RECOVERY_SECRET_STORE_CLASS: &str = "recoverySecret";
pub(super) const MLS_PUBLIC_STATE_DIGEST_AUTHENTICATED_BACKFILL: &str =
    "pending:selected-custody-authenticated-backfill";
pub(crate) const MLS_CAPABILITY_EXTENSION_TYPE_ID: u16 = 0xff10;
pub(crate) const MLS_CAPABILITY_EXTENSION_SCHEMA_VERSION: u32 = 2;
