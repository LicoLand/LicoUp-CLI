// The collection document contract is owned by the published store so the
// client and every other collection consumer read and write one schema.
pub use collection_store::{CLIENT_STATE_DIR, MAX_COLLECTION_DOCUMENT_BYTES};

// The migration diagnostic reads this literal marker; the compile-time check
// below makes a divergence from the published owner a build error rather than
// a probe that silently compares documents against a stale constant.
pub const STATE_SCHEMA_VERSION: &str = "v0.0.1:schema:definition-1";

const _: () = assert!(const_bytes_eq(
    STATE_SCHEMA_VERSION.as_bytes(),
    collection_store::STATE_SCHEMA_VERSION.as_bytes(),
));

const fn const_bytes_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

pub const ACTIVITY_DIR: &str = "activity";
pub const ACTIVITY_FILE: &str = "activity.jsonl";
pub const SNAPSHOT_DIR: &str = "snapshots";
pub const REDACTED_SECRET: &str = "<redacted-secret>";
pub const REDACTED_PRIVATE_KEY: &str = "<redacted-private-key>";
pub const REDACTED_LOCAL_PATH: &str = "<private-local-path>";
pub const MAX_ACTIVITY_FILE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_ACTIVITY_EVENT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_ACTIVITY_EVENTS: usize = 10_000;
pub const MAX_ACTIVITY_TYPE_BYTES: usize = 128;
pub const MAX_SNAPSHOT_SOURCE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SNAPSHOT_RECORD_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_SNAPSHOT_FILES: usize = 10_000;
pub const MAX_SNAPSHOT_ID_BYTES: usize = 192;
pub const MAX_LOCAL_PATH_BYTES: usize = 4 * 1024;
pub const MAX_REDACTION_DEPTH: usize = 64;
pub const MAX_REDACTION_PATHS: usize = 4_096;

pub const COLLECTIONS: &[&str] = &[
    "settings",
    "targets",
    "target-discovery-cache",
    "pairings",
    "skills",
    "pins",
    "identities",
    "conversation-archive-profiles",
    "agent-usage-reports",
    "provider-quota-snapshots",
    "skill-usage",
    "collaboration-plugins",
    "local-server-assemblies",
    "local-server-assembly-cleanup",
    "local-server-assembly-transaction",
    "mcp-install-transactions",
];
