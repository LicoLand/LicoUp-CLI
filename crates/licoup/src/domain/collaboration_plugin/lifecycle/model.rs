use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(in crate::domain::collaboration_plugin) struct CapabilityState {
    pub(in crate::domain::collaboration_plugin) schema_version: String,
    pub(in crate::domain::collaboration_plugin) capability_enabled: bool,
    pub(in crate::domain::collaboration_plugin) installed: Option<InstalledPlugin>,
    pub(super) cleanup_pending: Vec<PendingCleanup>,
    pub(super) cancelled_install_plans: Vec<CancelledInstallPlan>,
    pub(in crate::domain::collaboration_plugin) runner_trust: Option<RunnerTrustRecord>,
    /// Cached projection only. The append-only, user-presence protected
    /// platform ledger remains authoritative.
    pub(in crate::domain::collaboration_plugin) authority_record:
        Option<crate::core::authorized_secure_record::VersionedSecureRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(in crate::domain::collaboration_plugin) struct RunnerTrustRecord {
    pub(in crate::domain::collaboration_plugin) key_id: String,
    pub(in crate::domain::collaboration_plugin) public_key_base64url: String,
    pub(in crate::domain::collaboration_plugin) fingerprint_sha256: String,
    pub(in crate::domain::collaboration_plugin) source_repository_url: String,
    pub(in crate::domain::collaboration_plugin) runner_identity: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct PendingCleanup {
    pub(super) kind: String,
    pub(super) entry_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct CancelledInstallPlan {
    pub(super) plan_id: String,
    pub(super) digest_sha256: String,
    pub(super) expires_at_epoch_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(in crate::domain::collaboration_plugin) struct InstalledPlugin {
    pub(in crate::domain::collaboration_plugin) plugin_id: String,
    pub(in crate::domain::collaboration_plugin) display_name: String,
    pub(in crate::domain::collaboration_plugin) version: String,
    pub(in crate::domain::collaboration_plugin) digest_sha256: String,
    pub(in crate::domain::collaboration_plugin) capabilities: Vec<String>,
    pub(in crate::domain::collaboration_plugin) source_url: String,
    pub(in crate::domain::collaboration_plugin) source_commit_oid: String,
    pub(in crate::domain::collaboration_plugin) signed_package_inventory_digest_sha256: String,
    pub(in crate::domain::collaboration_plugin) runner_trust_key_id: String,
    pub(in crate::domain::collaboration_plugin) runner_trust_public_key_base64url: String,
    pub(in crate::domain::collaboration_plugin) runner_trust_fingerprint_sha256: String,
    pub(in crate::domain::collaboration_plugin) runner_platform: String,
    pub(in crate::domain::collaboration_plugin) runner_architecture: String,
    pub(in crate::domain::collaboration_plugin) runner_relative_path: String,
    pub(in crate::domain::collaboration_plugin) runner_digest_sha256: String,
    pub(in crate::domain::collaboration_plugin) runner_contract_version: String,
    pub(in crate::domain::collaboration_plugin) health_contract_version: String,
    pub(in crate::domain::collaboration_plugin) capabilities_contract_version: String,
}

#[derive(Clone, Debug)]
pub(in crate::domain::collaboration_plugin) struct InstalledWorkflowPlugin {
    pub(in crate::domain::collaboration_plugin) plugin_id: String,
    pub(in crate::domain::collaboration_plugin) digest_sha256: String,
    pub(in crate::domain::collaboration_plugin) version: String,
    pub(in crate::domain::collaboration_plugin) source_url: String,
    pub(in crate::domain::collaboration_plugin) package_root: PathBuf,
    pub(in crate::domain::collaboration_plugin) source_commit_oid: String,
    pub(in crate::domain::collaboration_plugin) signed_package_inventory_digest_sha256: String,
    pub(in crate::domain::collaboration_plugin) runner_trust_key_id: String,
    pub(in crate::domain::collaboration_plugin) runner_trust_public_key_base64url: String,
    pub(in crate::domain::collaboration_plugin) runner_trust_fingerprint_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct InstallPlanRecord {
    pub(super) schema_version: String,
    pub(super) plan_id: String,
    pub(super) source_url: String,
    pub(super) source_ref: Option<String>,
    pub(super) plugin_path: Option<String>,
    pub(super) plugin_id: String,
    pub(super) display_name: String,
    pub(super) version: String,
    pub(super) digest_sha256: String,
    pub(super) capabilities: Vec<String>,
    pub(super) file_count: usize,
    pub(super) total_bytes: usize,
    pub(super) created_at_epoch_seconds: u64,
    pub(super) expires_at_epoch_seconds: u64,
    pub(super) signed_package_inventory_digest_sha256: String,
    pub(super) runner_trust_key_id: String,
    pub(super) runner_trust_public_key_base64url: String,
    pub(super) runner_trust_fingerprint_sha256: String,
    pub(super) runner_platform: String,
    pub(super) runner_architecture: String,
    pub(super) runner_relative_path: String,
    pub(super) runner_digest_sha256: String,
    pub(super) runner_contract_version: String,
    pub(super) health_contract_version: String,
    pub(super) capabilities_contract_version: String,
}
