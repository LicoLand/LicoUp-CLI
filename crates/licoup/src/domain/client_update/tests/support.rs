#[cfg(target_os = "macos")]
pub(super) use super::super::verify::verify_staged_selection;
pub(super) use super::super::{
    apply::apply,
    canonical::{sha256_hex, stable_stringify, unsigned_document},
    check::check,
    constants::{
        CLIENT_UPDATE_ARTIFACT_RECEIPT_SCHEMA, CLIENT_UPDATE_MANIFEST_SCHEMA,
        CLIENT_UPDATE_REVOCATION_SCHEMA,
    },
    dispatch::dispatch,
    download::download,
    status::status,
    verify::verify,
};
pub(super) use base64::{Engine as _, engine::general_purpose};
pub(super) use ed25519_dalek::{Signer, SigningKey};
pub(super) use rand_core::OsRng;
pub(super) use serde_json::{Value, json};
pub(super) use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) const OFFLINE_KEY_ID: &str = "offline-root-test";
pub(super) const ONLINE_KEY_ID: &str = "online-signing-test";
pub(super) const TARGET_ID: &str = "test-target";

pub(super) struct UpdateFixture {
    pub root: PathBuf,
    pub source: PathBuf,
    pub staging: PathBuf,
    pub offline: SigningKey,
    pub online: SigningKey,
}

impl UpdateFixture {
    pub(super) fn new() -> Self {
        let root = std::env::temp_dir().join(format!("licoup-update-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("artifact.bin");
        fs::write(&source, b"signed-update-artifact").unwrap();
        Self {
            staging: root.join("staging"),
            root,
            source,
            offline: SigningKey::generate(&mut OsRng),
            online: SigningKey::generate(&mut OsRng),
        }
    }

    pub(super) fn artifact(&self, target_id: &str) -> Value {
        json!({
            "targetId": target_id,
            "platform": "test",
            "osFamily": "test",
            "arch": "test",
            "installerStrategy": "portable-replacement",
            "url": url::Url::from_file_path(&self.source).unwrap().to_string(),
            "fileName": "artifact.bin",
            "size": fs::metadata(&self.source).unwrap().len(),
            "sha256": sha256_hex(&fs::read(&self.source).unwrap()),
        })
    }

    pub(super) fn unsigned_manifest(&self, releases: Value) -> Value {
        json!({
            "schemaVersion": CLIENT_UPDATE_MANIFEST_SCHEMA,
            "releaseTrack": "stable",
            "releaseTrackPolicy": {
                "offlineRootKeyId": OFFLINE_KEY_ID,
                "onlineSigningKeyId": ONLINE_KEY_ID,
            },
            "releases": releases,
        })
    }

    pub(super) fn manifest(&self) -> Value {
        self.sign_manifest(self.unsigned_manifest(json!([{
            "version": "999.0.0",
            "minimumSupportedVersion": "0.0.0",
            "classification": "optional",
            "releaseNotesUrl": "https://updates.invalid/999.0.0",
            "migrationNotes": [],
            "migrationFrontier": crate::domain::client_state_migration::frontier_projection().unwrap(),
            "artifacts": [self.artifact(TARGET_ID)],
        }])))
    }

    pub(super) fn sign_manifest(&self, manifest: Value) -> Value {
        sign_document(
            manifest,
            &[
                (OFFLINE_KEY_ID, &self.offline),
                (ONLINE_KEY_ID, &self.online),
            ],
        )
    }

    pub(super) fn public_keys(&self) -> Value {
        json!({
            "keys": {
                OFFLINE_KEY_ID: {
                    "publicKey": general_purpose::STANDARD.encode(self.offline.verifying_key().as_bytes())
                },
                ONLINE_KEY_ID: {
                    "publicKey": general_purpose::STANDARD.encode(self.online.verifying_key().as_bytes())
                }
            }
        })
    }

    pub(super) fn params(&self, manifest: Value) -> Value {
        json!({
            "manifestJson": manifest,
            "publicKeys": self.public_keys(),
            "targetReleaseTrack": "stable",
            "targetId": TARGET_ID,
            "sourcePath": self.source,
            "stagingRoot": self.staging,
            "stateRoot": self.root.join("state"),
        })
    }

    pub(super) fn checked_params(&self, manifest: Value) -> Value {
        let mut params = self.params(manifest);
        super::super::check::check(&params).unwrap();
        params
            .as_object_mut()
            .expect("test params must be an object")
            .remove("targetReleaseTrack");
        params
    }

    pub(super) fn signed_revocation(&self, body: Value) -> Value {
        sign_document(body, &[(OFFLINE_KEY_ID, &self.offline)])
    }
}

impl Drop for UpdateFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub(super) fn sign_document(mut document: Value, keys: &[(&str, &SigningKey)]) -> Value {
    if let Some(object) = document.as_object_mut() {
        object.remove("signatures");
    }
    let payload = stable_stringify(&unsigned_document(&document));
    let signatures = keys
        .iter()
        .map(|(key_id, key)| {
            let signature = key.sign(payload.as_bytes());
            json!({
                "keyId": key_id,
                "algorithm": "Ed25519",
                "signature": general_purpose::STANDARD.encode(signature.to_bytes()),
            })
        })
        .collect::<Vec<_>>();
    document["signatures"] = Value::Array(signatures);
    document
}

pub(super) fn release(version: &str, artifact: Value) -> Value {
    json!({
        "version": version,
        "minimumSupportedVersion": "0.0.0",
        "classification": "optional",
        "releaseNotesUrl": format!("https://updates.invalid/{version}"),
        "migrationNotes": [],
        "migrationFrontier": crate::domain::client_state_migration::frontier_projection().unwrap(),
        "artifacts": [artifact],
    })
}

pub(super) fn revocation_body() -> Value {
    json!({
        "schemaVersion": CLIENT_UPDATE_REVOCATION_SCHEMA,
        "releaseTrack": "stable",
        "offlineRootKeyId": OFFLINE_KEY_ID,
        "revokedKeyIds": [],
        "revokedVersions": [],
        "revokedArtifactDigests": [],
    })
}

pub(super) fn assert_redacted(value: &Value, root: &Path) {
    let serialized = value.to_string();
    assert!(!serialized.contains(&root.to_string_lossy().to_string()));
    assert!(value.get("installedAppPath").is_none());
    assert!(value.get("stagedAppPath").is_none());
    assert!(value.get("sourcePath").is_none());
}
