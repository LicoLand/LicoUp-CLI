use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

use super::manifest::{
    SERVER_CAPABILITIES_CONTRACT, SERVER_HEALTH_CONTRACT, SERVER_RUNNER_CONTRACT,
    current_server_runner_target, expected_server_runner_path, parse_manifest,
};
use super::package::{PackageFile, signed_inventory_digest};

pub(super) const TEST_SOURCE_URL: &str = "https://github.com/example/collaboration-plugin.git";
pub(super) const TEST_COMMIT_OID: &str = "0123456789abcdef0123456789abcdef01234567";

pub(super) fn finalize_signed_test_manifest(root: &Path, mut manifest: Value) {
    let runner_bytes = b"synthetic-fixed-runner";
    let (platform, architecture) = current_server_runner_target().unwrap();
    let relative = expected_server_runner_path(platform, architecture);
    fs::create_dir_all(root.join(&relative).parent().unwrap()).unwrap();
    fs::write(root.join(&relative), runner_bytes).unwrap();
    manifest["signedPackageInventoryDigestSha256"] = json!("0".repeat(64));
    manifest["serverRunners"] = json!([{
        "sourceUrl": TEST_SOURCE_URL,
        "sourceCommitOid": TEST_COMMIT_OID,
        "platform": platform,
        "architecture": architecture,
        "relativePath": super::manifest::normalized_relative_protocol_path(&relative).unwrap(),
        "digestSha256": format!("{:x}", Sha256::digest(runner_bytes)),
        "runnerContractVersion": SERVER_RUNNER_CONTRACT,
        "healthContractVersion": SERVER_HEALTH_CONTRACT,
        "capabilitiesContractVersion": SERVER_CAPABILITIES_CONTRACT,
        "signatureBase64url": URL_SAFE_NO_PAD.encode([0u8; 64])
    }]);
    write_manifest(root, &manifest);
    let inventory = signed_inventory_digest(&collect_files(root)).unwrap();
    manifest["signedPackageInventoryDigestSha256"] = json!(inventory);
    write_manifest(root, &manifest);
    let parsed =
        parse_manifest(&fs::read(root.join(super::manifest::MANIFEST_FILE)).unwrap()).unwrap();
    let signature = super::runner_signature::sign_runner_for_test(
        &parsed,
        parsed.server_runners.first().unwrap(),
    );
    manifest["serverRunners"][0]["signatureBase64url"] = json!(signature);
    write_manifest(root, &manifest);
}

pub(super) fn import_test_runner_trust(store: &crate::platform::client_state::ClientStateStore) {
    let (key_id, public_key, fingerprint) = super::runner_signature::test_trust();
    super::lifecycle::runner_trust_import_in(
        store,
        &json!({
            "requestOrigin": "direct-user",
            "runnerTrustKeyId": key_id,
            "runnerTrustPublicKeyBase64url": public_key,
            "expectedRunnerTrustFingerprintSha256": fingerprint,
            "runnerSourceRepositoryUrl": TEST_SOURCE_URL,
            "runnerIdentity": super::runner_signature::OFFICIAL_SERVER_RUNNER_IDENTITY,
            "confirmed": true
        }),
    )
    .unwrap();
}

fn write_manifest(root: &Path, manifest: &Value) {
    fs::write(
        root.join(super::manifest::MANIFEST_FILE),
        serde_json::to_vec(manifest).unwrap(),
    )
    .unwrap();
}

fn collect_files(root: &Path) -> Vec<PackageFile> {
    fn visit(root: &Path, directory: &Path, files: &mut Vec<PackageFile>) {
        let mut entries = fs::read_dir(directory)
            .unwrap()
            .collect::<std::io::Result<Vec<_>>>()
            .unwrap();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, files);
            } else {
                files.push(PackageFile {
                    relative_path: path.strip_prefix(root).unwrap().to_path_buf(),
                    bytes: fs::read(path).unwrap(),
                });
            }
        }
    }
    let mut files = Vec::new();
    visit(root, root, &mut files);
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    files
}
