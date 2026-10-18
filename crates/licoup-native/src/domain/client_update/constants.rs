pub const CLIENT_UPDATE_MANIFEST_SCHEMA: &str = "v0.0.1:client-update:manifest-2";
pub const CLIENT_UPDATE_REVOCATION_SCHEMA: &str = "v0.0.1:client-update:revocation-list-2";
pub const CLIENT_UPDATE_ARTIFACT_RECEIPT_SCHEMA: &str = "v0.0.1:client-update:artifact-receipt-2";
pub const CLIENT_UPDATE_MODE: &str = "client-update";

pub(super) const MAX_UPDATE_METADATA_BYTES: u64 = 1024 * 1024;
pub(super) const UPDATE_COPY_BUFFER_BYTES: usize = 64 * 1024;
