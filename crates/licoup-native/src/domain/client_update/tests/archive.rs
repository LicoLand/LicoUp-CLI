use super::super::{
    archive::extract_signed_archive,
    archive::validate_archive_path_for_test,
    canonical::sha256_hex,
    model::{VerifiedArtifact, VerifiedUpdateSelection},
};
use super::support::TARGET_ID;

use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};
use zip::write::SimpleFileOptions;

#[test]
fn client_update_archive_paths_reject_parent_absolute_and_current_components() {
    for path in [
        Path::new("../escape"),
        Path::new("/absolute"),
        Path::new("./current"),
    ] {
        assert!(
            validate_archive_path_for_test(path)
                .unwrap_err()
                .to_string()
                .contains("relative and normalized")
        );
    }
}

#[test]
fn client_update_zip_extraction_expands_files_within_the_extraction_root() {
    let root = temporary_root("licoup-update-zip-ok");
    let archive_path = root.join("client-update.zip");
    write_zip_entries(
        &archive_path,
        &[
            ZipEntry::Directory("LicoUp.app".to_string()),
            ZipEntry::Directory("LicoUp.app/Contents".to_string()),
            ZipEntry::File(
                "LicoUp.app/Contents/Info.plist".to_string(),
                b"staged".to_vec(),
            ),
        ],
    );
    let selection = selection_for(&archive_path, "client-update.zip");
    let extraction = root.join("expanded");
    fs::create_dir_all(&extraction).unwrap();
    extract_signed_archive(&selection, &archive_path, &extraction).unwrap();
    assert_eq!(
        fs::read(extraction.join("LicoUp.app/Contents/Info.plist")).unwrap(),
        b"staged"
    );
    assert!(extraction.join("LicoUp.app/Contents").is_dir());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn client_update_zip_extraction_rejects_traversal_entries() {
    let root = temporary_root("licoup-update-zip-traversal");
    let archive_path = root.join("client-update.zip");
    write_zip_entries(
        &archive_path,
        &[ZipEntry::File("../escape".to_string(), b"boom".to_vec())],
    );
    let selection = selection_for(&archive_path, "client-update.zip");
    let extraction = root.join("expanded");
    fs::create_dir_all(&extraction).unwrap();
    let error = extract_signed_archive(&selection, &archive_path, &extraction)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("relative and normalized")
            || error.contains("escapes")
            || error.contains("entry path is invalid"),
        "unexpected error: {error}"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn client_update_zip_extraction_accepts_only_contained_relative_symbolic_links() {
    let root = temporary_root("licoup-update-zip-contained-link");
    let archive_path = root.join("client-update.zip");
    write_zip_entries(
        &archive_path,
        &[
            ZipEntry::Directory("LicoUp.app/Versions/A".to_string()),
            ZipEntry::File(
                "LicoUp.app/Versions/A/resource".to_string(),
                b"safe".to_vec(),
            ),
            ZipEntry::Symlink("LicoUp.app/Versions/Current".to_string(), "A".to_string()),
        ],
    );
    let selection = selection_for(&archive_path, "client-update.zip");
    let extraction = root.join("expanded");
    fs::create_dir_all(&extraction).unwrap();
    extract_signed_archive(&selection, &archive_path, &extraction).unwrap();
    assert_eq!(
        fs::read(extraction.join("LicoUp.app/Versions/Current/resource")).unwrap(),
        b"safe"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn client_update_zip_extraction_rejects_escaping_symbolic_link_entries() {
    let root = temporary_root("licoup-update-zip-link");
    let archive_path = root.join("client-update.zip");
    write_zip_entries(
        &archive_path,
        &[ZipEntry::Symlink(
            "LicoUp.app/Contents/Info.plist".to_string(),
            "../escape".to_string(),
        )],
    );
    let selection = selection_for(&archive_path, "client-update.zip");
    let extraction = root.join("expanded");
    fs::create_dir_all(&extraction).unwrap();
    let error = extract_signed_archive(&selection, &archive_path, &extraction)
        .unwrap_err()
        .to_string();
    assert!(error.contains("relative and normalized") || error.contains("escapes"));
    fs::remove_dir_all(&root).unwrap();
}

enum ZipEntry {
    Directory(String),
    File(String, Vec<u8>),
    Symlink(String, String),
}

fn write_zip_entries(path: &Path, entries: &[ZipEntry]) {
    let file = fs::File::create(path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    for entry in entries {
        match entry {
            ZipEntry::Directory(name) => writer.add_directory(name, options).unwrap(),
            ZipEntry::File(name, content) => {
                writer.start_file(name, options).unwrap();
                writer.write_all(content).unwrap();
            }
            ZipEntry::Symlink(name, target) => writer.add_symlink(name, target, options).unwrap(),
        }
    }
    writer.finish().unwrap();
}

fn selection_for(path: &Path, file_name: &str) -> VerifiedUpdateSelection {
    VerifiedUpdateSelection {
        running_release_track: "nightly".to_string(),
        target_release_track: "stable".to_string(),
        running_version: "0.0.1".to_string(),
        version: "999.0.0".to_string(),
        migration_frontier: crate::domain::client_state_migration::frontier_projection().unwrap(),
        classification: serde_json::json!("optional"),
        release_notes_url: serde_json::json!("https://updates.invalid/999.0.0"),
        migration_notes: serde_json::json!([]),
        verified_key_ids: Vec::new(),
        manifest_sha256: String::new(),
        artifact: VerifiedArtifact {
            target_id: TARGET_ID.to_string(),
            platform: "test".to_string(),
            os_family: "test".to_string(),
            arch: "test".to_string(),
            installer_strategy: "portable-replacement".to_string(),
            url: "https://updates.invalid/client-update.zip".to_string(),
            file_name: file_name.to_string(),
            size: fs::metadata(path).unwrap().len(),
            sha256: sha256_hex(&fs::read(path).unwrap()),
            application_name: None,
            bundle_id: None,
        },
    }
}

fn temporary_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("{label}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    root
}
