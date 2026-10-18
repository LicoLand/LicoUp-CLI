/// The product path is wired through typed native actions and selected custody. Release evidence
/// remains pending until the requested physical multi-client matrix is accepted.
pub const SECURE_MESH_MLS_PRODUCT_POLICY_STATUS: &str = "cryptographic_native_path_wired_local_persisted_trust_and_authorized_directory_leaf_kt_authority_physical_matrix_pending";

pub(super) const MLS_CREDENTIAL_MAGIC: &[u8] = b"LCOSM-MLS-CRED-v1";
pub(super) const MAX_ROSTER: usize = 256;
pub(super) const MAX_EPOCH_LAG: u64 = 2;
pub const SECURE_MESH_MLS_CAPABILITY_POLICY_REVISION: u64 = 2;

pub(super) const MAX_PERSISTED_MLS_CAPABILITY_PROOFS: usize = 4096;
pub(super) const MAX_PERSISTED_MLS_KEY_PACKAGES_PER_SCOPE: usize = 4096;
pub(super) const MAX_INCOMPLETE_MLS_OPERATIONS_PER_SCOPE: usize = 16;
pub(super) const MAX_DELIVERED_MLS_OPERATIONS_PER_SCOPE: usize = 256;
pub(super) const STALE_EMPTY_PREPARED_OPERATION_SECONDS: i64 = 86_400;
pub(super) const MLS_SECURITY_LEDGER_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS secure_mesh_mls_keypackage_uses (
    consumer_endpoint_id TEXT NOT NULL,
    key_package_id TEXT NOT NULL,
    key_package_public_key_hash TEXT NOT NULL,
    group_id_hash TEXT NOT NULL,
    used_at TEXT NOT NULL,
    PRIMARY KEY (consumer_endpoint_id, key_package_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS secure_mesh_mls_keypackage_pubkey_hash_uq
    ON secure_mesh_mls_keypackage_uses (consumer_endpoint_id, key_package_public_key_hash);
CREATE TABLE IF NOT EXISTS secure_mesh_mls_capability_proof_uses (
    local_endpoint_scope_hash TEXT NOT NULL,
    proof_digest TEXT NOT NULL,
    expires_at_unix_seconds INTEGER NOT NULL,
    consumed_at_unix_seconds INTEGER NOT NULL,
    PRIMARY KEY (local_endpoint_scope_hash, proof_digest)
);
CREATE INDEX IF NOT EXISTS secure_mesh_mls_capability_proof_expiry_idx
    ON secure_mesh_mls_capability_proof_uses(expires_at_unix_seconds);
CREATE TABLE IF NOT EXISTS secure_mesh_mls_time_guard (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    max_observed_unix_seconds INTEGER NOT NULL CHECK (max_observed_unix_seconds >= 0)
);
INSERT OR IGNORE INTO secure_mesh_mls_time_guard(singleton, max_observed_unix_seconds)
    VALUES(1, 0);
CREATE TABLE IF NOT EXISTS secure_mesh_mls_operations (
    operation_id TEXT PRIMARY KEY,
    local_endpoint_scope_hash TEXT NOT NULL,
    action TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    state TEXT NOT NULL,
    response_json TEXT,
    group_id_base64url TEXT,
    base_metadata_json TEXT,
    expected_metadata_json TEXT,
    prepared_security_json TEXT,
    created_at_unix_seconds INTEGER NOT NULL,
    updated_at_unix_seconds INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS secure_mesh_mls_operation_reservations (
    local_endpoint_scope_hash TEXT NOT NULL,
    reservation_key TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    PRIMARY KEY (local_endpoint_scope_hash, reservation_key),
    FOREIGN KEY (operation_id) REFERENCES secure_mesh_mls_operations(operation_id)
        ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS secure_mesh_mls_operation_state_idx
    ON secure_mesh_mls_operations(state, updated_at_unix_seconds);
"#;
