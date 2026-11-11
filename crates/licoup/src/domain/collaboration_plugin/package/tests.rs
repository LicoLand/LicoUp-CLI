use super::*;
use serde_json::json;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn fixture_root(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("lico-collaboration-package-{name}-{stamp}"))
}

fn write_fixture(root: &Path) {
    fs::create_dir_all(root.join("workflows")).unwrap();
    fs::create_dir_all(root.join("payload/server-core")).unwrap();
    fs::create_dir_all(root.join("payload/mcp-selected")).unwrap();
    let manifest = json!({
        "schemaVersion": super::super::manifest::MANIFEST_SCHEMA,
        "kind": super::super::manifest::PLUGIN_KIND,
        "pluginId": "licomesh-collaboration",
        "displayName": "LicoMesh Collaboration",
        "version": "1.0.0",
        "capabilities": [
            super::super::manifest::LOCAL_DEPLOYMENT_CAPABILITY,
            super::super::manifest::MCP_INSTALL_CAPABILITY
        ],
        "workflows": {
            "localDeployment": "workflows/local-deployment.json",
            "mcpInstall": "workflows/mcp-install.json"
        }
    });
    fs::write(
            root.join("workflows/local-deployment.json"),
            br#"{"schemaVersion":"licoup.collaboration.local-deployment.v1","manualOnly":true,"features":[{"id":"server-core","label":"Server Core","packagePath":"payload/server-core"}]}"#,
        )
        .unwrap();
    fs::write(
            root.join("workflows/mcp-install.json"),
            br#"{"schemaVersion":"licoup.collaboration.mcp-install.v2","manualOnly":true,"plugins":[{"id":"selected","label":"Selected MCP","packagePath":"payload/mcp-selected","endpoint":"https://example.invalid/mcp"}],"requiresPerFileApproval":true,"outboundPolicy":"direct-user-exact-scope-one-shot"}"#,
        )
        .unwrap();
    fs::write(root.join("payload/server-core/package.json"), b"{}").unwrap();
    fs::write(root.join("payload/mcp-selected/package.json"), b"{}").unwrap();
    super::super::test_support::finalize_signed_test_manifest(root, manifest);
}

#[test]
fn package_digest_is_deterministic_and_copy_is_non_executable() {
    let source = fixture_root("deterministic-source");
    let destination = fixture_root("deterministic-destination");
    write_fixture(&source);
    let inspected = inspect_package(&source).unwrap();
    write_inspected_package(&inspected, &destination).unwrap();
    let copied = inspect_package(&destination).unwrap();
    assert_eq!(copied.digest_sha256, inspected.digest_sha256);
    assert_eq!(copied.file_count, inspected.file_count);
    let _ = fs::remove_dir_all(source);
    let _ = fs::remove_dir_all(destination);
}

#[test]
fn package_copy_never_overwrites_a_preexisting_destination_sentinel() {
    let source = fixture_root("preexisting-source");
    let destination = fixture_root("preexisting-destination");
    write_fixture(&source);
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("sentinel"), b"preserve").unwrap();
    let inspected = inspect_package(&source).unwrap();
    assert!(write_inspected_package(&inspected, &destination).is_err());
    assert_eq!(fs::read(destination.join("sentinel")).unwrap(), b"preserve");
    let _ = fs::remove_dir_all(source);
    let _ = fs::remove_dir_all(destination);
}

#[cfg(unix)]
#[test]
fn concurrent_destination_replacement_cannot_redirect_package_writes() {
    use std::os::unix::fs::symlink;

    let source = fixture_root("replacement-source");
    let destination = fixture_root("replacement-destination");
    let outside = fixture_root("replacement-outside");
    write_fixture(&source);
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("sentinel"), b"preserve").unwrap();
    let inspected = inspect_package(&source).unwrap();
    assert!(
        write_inspected_package_with_hook(&inspected, &destination, || {
            fs::remove_dir(&destination)?;
            symlink(&outside, &destination)?;
            Ok(())
        },)
        .is_err()
    );
    assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"preserve");
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
    let _ = fs::remove_file(destination);
    let _ = fs::remove_dir_all(source);
    let _ = fs::remove_dir_all(outside);
}

#[test]
fn workflow_descriptor_cannot_smuggle_an_executable_directive() {
    let source = fixture_root("executable-directive");
    write_fixture(&source);
    fs::write(
            source.join("workflows/local-deployment.json"),
            br#"{"schemaVersion":"licoup.collaboration.local-deployment.v1","manualOnly":true,"features":[{"id":"server-core","label":"Server Core","packagePath":"payload/server-core"}],"command":"run-me"}"#,
        )
        .unwrap();
    assert!(inspect_package(&source).is_err());
    let _ = fs::remove_dir_all(source);
}

#[cfg(unix)]
#[test]
fn package_rejects_symlinks_without_following_them() {
    use std::os::unix::fs::symlink;
    let source = fixture_root("symlink-source");
    let outside = fixture_root("symlink-outside");
    write_fixture(&source);
    fs::write(&outside, b"outside").unwrap();
    symlink(&outside, source.join("outside-link")).unwrap();
    assert!(inspect_package(&source).is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"outside");
    let _ = fs::remove_dir_all(source);
    let _ = fs::remove_file(outside);
}
