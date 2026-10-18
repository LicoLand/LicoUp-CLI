//! Protocol limits, domain separators, and stable status markers.

pub const SECURE_MESH_TRANSPARENCY_STATUS: &str =
    "verification_only_pinned_log_rfc9162_sparse_directory_sqlite_cas_gossip_fail_closed";
pub const SECURE_MESH_KT_PROTOCOL_VERSION: &str = "licomesh.secure-mesh.kt.v2";
pub const SECURE_MESH_KT_GOSSIP_CONTENT_TYPE: &str =
    "application/vnd.licomesh.secure-mesh.kt-sth-gossip+json";
pub const KT_PROTOCOL_MAX_STH_AGE_SECONDS: u64 = 24 * 60 * 60;
pub const KT_PROTOCOL_MAX_FUTURE_SKEW_SECONDS: u64 = 5 * 60;
pub const KT_PROTOCOL_MAX_GOSSIP_AGE_SECONDS: u64 = 15 * 60;
pub const KT_JSON_SAFE_INTEGER_MAX: u64 = 9_007_199_254_740_991;

pub(super) const STH_SIGN_MAGIC: &[u8] = b"LCOSM-KT-STH-v2";
pub(super) const DIRECTORY_SCOPE_COMMITMENT_DOMAIN: &[u8] = b"LCOSM-KT-DIRECTORY-SCOPE-v1";
pub(super) const DIRECTORY_LABEL_DOMAIN: &[u8] = b"LCOSM-KT-DIRECTORY-LABEL-v1";
pub(super) const MAP_KEY_DOMAIN: &[u8] = b"LCOSM-KT-MAP-KEY-v1";
pub(super) const MAP_LEAF_DOMAIN: &[u8] = b"LCOSM-KT-MAP-LEAF-v1";
pub(super) const MAP_EMPTY_DOMAIN: &[u8] = b"LCOSM-KT-MAP-EMPTY-v1";
pub(super) const MAP_NODE_DOMAIN: &[u8] = b"LCOSM-KT-MAP-NODE-v1";
pub(super) const COMBINED_MAP_ROOT_LOG_ENTRY_DOMAIN: &[u8] = b"LCOSM-KT-COMBINED-MAP-ROOT-v1";
pub(super) const MAX_TRANSPARENCY_FIELD_BYTES: usize = 8_192;
#[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
pub(super) const MAX_TRANSPARENCY_LEAVES: usize = 100_000;
pub(super) const MAX_INCLUSION_PROOF_HASHES: usize = 64;
pub(super) const MAX_CONSISTENCY_PROOF_HASHES: usize = 65;
pub(super) const MAX_PERSISTED_CHECKPOINTS: u64 = 64;
pub(super) const MAX_PERSISTED_GOSSIP_OBSERVATIONS: u64 = 64;
pub(super) const MAX_PERSISTED_DIRECTORY_LABELS: u64 = 4_096;
pub(super) const MAX_PERSISTED_DIRECTORY_AUTHORIZATIONS: u64 = 8_192;
pub(super) const KT_SCHEMA_VERSION: i64 = 7;
pub(super) const SPARSE_MAP_DEPTH: usize = 256;
pub(super) const HASH_LEN: usize = 32;

pub const SECURE_MESH_DIAGNOSTIC_HASH_CHAIN_STATUS: &str =
    "diagnostic_only_unsigned_append_only_hash_chain_non_authorizing";
