use serde_json::{Value, json};

use super::{
    canonical::{canonical_unsigned_sha256, sha256_hex, stable_stringify},
    constants::CLIENT_UPDATE_ARTIFACT_RECEIPT_SCHEMA,
};

#[derive(Clone, Debug)]
pub(super) struct VerifiedArtifact {
    pub target_id: String,
    pub platform: String,
    pub os_family: String,
    pub arch: String,
    pub installer_strategy: String,
    pub url: String,
    pub file_name: String,
    pub size: u64,
    pub sha256: String,
    pub application_name: Option<String>,
    pub bundle_id: Option<String>,
}

impl VerifiedArtifact {
    pub(super) fn public_projection(&self) -> Value {
        json!({
            "targetId": self.target_id,
            "platform": self.platform,
            "osFamily": self.os_family,
            "arch": self.arch,
            "installerStrategy": self.installer_strategy,
            "fileName": self.file_name,
            "size": self.size,
            "sha256": self.sha256,
        })
    }
}

#[derive(Clone, Debug)]
pub(super) struct VerifiedUpdateSelection {
    pub running_release_track: String,
    pub target_release_track: String,
    pub running_version: String,
    pub version: String,
    pub migration_frontier: Value,
    pub classification: Value,
    pub release_notes_url: Value,
    pub migration_notes: Value,
    pub verified_key_ids: Vec<String>,
    pub manifest_sha256: String,
    pub artifact: VerifiedArtifact,
}

impl VerifiedUpdateSelection {
    pub(super) fn receipt(&self) -> Value {
        let binding = json!({
            "schemaVersion": CLIENT_UPDATE_ARTIFACT_RECEIPT_SCHEMA,
            "manifestSha256": self.manifest_sha256,
            "runningReleaseTrack": self.running_release_track,
            "targetReleaseTrack": self.target_release_track,
            "migrationFrontier": self.migration_frontier,
            "version": self.version,
            "targetId": self.artifact.target_id,
            "fileName": self.artifact.file_name,
            "size": self.artifact.size,
            "sha256": self.artifact.sha256,
        });
        let receipt_id = sha256_hex(stable_stringify(&binding).as_bytes());
        let mut receipt = binding;
        receipt["receiptId"] = Value::String(receipt_id);
        receipt
    }
}

#[derive(Clone, Debug)]
pub(super) struct VerifiedManifest {
    pub running_release_track: String,
    pub target_release_track: String,
    pub running_version: String,
    pub verified_key_ids: Vec<String>,
    pub manifest_sha256: String,
    pub selected: Option<VerifiedUpdateSelection>,
}

impl VerifiedManifest {
    pub(super) fn from_selection(
        running_release_track: String,
        target_release_track: String,
        running_version: String,
        verified_key_ids: Vec<String>,
        manifest: &Value,
        artifact_and_release: Option<(VerifiedArtifact, &Value)>,
    ) -> Self {
        let manifest_sha256 = canonical_unsigned_sha256(manifest);
        let selected = artifact_and_release.map(|(artifact, release)| VerifiedUpdateSelection {
            running_release_track: running_release_track.clone(),
            target_release_track: target_release_track.clone(),
            running_version: running_version.clone(),
            version: release["version"].as_str().unwrap_or_default().to_string(),
            migration_frontier: release["migrationFrontier"].clone(),
            classification: release
                .get("classification")
                .cloned()
                .unwrap_or(Value::Null),
            release_notes_url: release
                .get("releaseNotesUrl")
                .cloned()
                .unwrap_or(Value::Null),
            migration_notes: release
                .get("migrationNotes")
                .cloned()
                .unwrap_or_else(|| json!([])),
            verified_key_ids: verified_key_ids.clone(),
            manifest_sha256: manifest_sha256.clone(),
            artifact,
        });
        Self {
            running_release_track,
            target_release_track,
            running_version,
            verified_key_ids,
            manifest_sha256,
            selected,
        }
    }
}
