//! Application owners prepare only verified scratch; Foundation alone publishes it.
use super::*;
use licoup_foundation::core::full_data_root_archive::restore_data_root_with_preparation;

struct OwnedRoot(PathBuf);
impl Drop for OwnedRoot {
    fn drop(&mut self) {
        fn remove(path: &Path) {
            let Ok(metadata) = fs::symlink_metadata(path) else {
                return;
            };
            if metadata.file_type().is_symlink() {
                let _ = fs::remove_file(path);
                return;
            }
            if metadata.permissions().readonly() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(
                        path,
                        fs::Permissions::from_mode(if metadata.is_dir() { 0o700 } else { 0o600 }),
                    );
                }
                #[cfg(not(unix))]
                {
                    let mut permissions = metadata.permissions();
                    permissions.set_readonly(false);
                    let _ = fs::set_permissions(path, permissions);
                }
            }
            if metadata.is_dir() {
                if let Ok(entries) = fs::read_dir(path) {
                    for entry in entries.flatten() {
                        remove(&entry.path());
                    }
                }
                let _ = fs::remove_dir(path);
            } else {
                let _ = fs::remove_file(path);
            }
        }
        remove(&self.0);
    }
}

fn freeze(path: &Path) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).unwrap();
}

fn fixture(label: &str, extension: &str) -> (OwnedRoot, PathBuf, PathBuf, PathBuf) {
    let root = OwnedRoot(scratch(&format!("owner-{label}-{extension}")));
    let source = root.0.join("source");
    write(&source.join("revision/content.json"), b"original");
    let archive = root.0.join(format!("source.{extension}"));
    export_data_root(&ExportRequest {
        data_root: source.clone(),
        archive_path: archive.clone(),
        writers_stopped: true,
    })
    .unwrap();
    let target = root.0.join("target");
    (root, source, archive, target)
}

#[test]
fn owners_prepare_before_publication_and_protections_survive_both_containers() {
    for extension in ["zip", "tar.gz"] {
        let (_root, source, archive, target) = fixture("positive", extension);
        let archived = fs::read(&archive).unwrap();
        restore_data_root_with_preparation(
            &RestoreRequest {
                archive_path: archive.clone(),
                target_root: target.clone(),
            },
            |staged, origin, destination| {
                assert_eq!(origin, source);
                assert_eq!(destination, target);
                assert_eq!(
                    fs::read_dir(&target)?.count(),
                    0,
                    "nothing published before owner checks"
                );
                assert!(!staged.starts_with(&target));
                assert_eq!(fs::read(staged.join("revision/content.json"))?, b"original");
                fs::write(
                    staged.join("revision/content.json"),
                    destination.to_string_lossy().as_bytes(),
                )?;
                write(&staged.join("prepared/metadata.json"), b"owner-produced");
                freeze(&staged.join("revision/content.json"));
                freeze(&staged.join("revision"));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            fs::read(target.join("revision/content.json")).unwrap(),
            target.to_string_lossy().as_bytes()
        );
        assert_eq!(
            fs::read(target.join("prepared/metadata.json")).unwrap(),
            b"owner-produced"
        );
        assert!(
            fs::metadata(target.join("revision"))
                .unwrap()
                .permissions()
                .readonly()
        );
        assert!(
            fs::metadata(target.join("revision/content.json"))
                .unwrap()
                .permissions()
                .readonly()
        );
        assert_eq!(
            fs::read(source.join("revision/content.json")).unwrap(),
            b"original"
        );
        assert_eq!(fs::read(&archive).unwrap(), archived);
    }
}

#[test]
fn refused_preparation_never_publishes_and_cleans_hardened_scratch() {
    for extension in ["zip", "tar.gz"] {
        for existing in [false, true] {
            let (root, source, archive, target) = fixture(
                if existing {
                    "refuse-existing"
                } else {
                    "refuse-created"
                },
                extension,
            );
            if existing {
                fs::create_dir(&target).unwrap();
            }
            let archived = fs::read(&archive).unwrap();
            let error = restore_data_root_with_preparation(
                &RestoreRequest {
                    archive_path: archive.clone(),
                    target_root: target.clone(),
                },
                |staged, _, _| {
                    assert_eq!(fs::read_dir(&target)?.count(), 0);
                    freeze(&staged.join("revision/content.json"));
                    freeze(&staged.join("revision"));
                    Err(anyhow::anyhow!("synthetic_owner_refused"))
                },
            )
            .unwrap_err();
            assert_eq!(error.to_string(), "synthetic_owner_refused");
            assert_eq!(target.exists(), existing);
            if existing {
                assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
            }
            assert!(!fs::read_dir(&root.0).unwrap().any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".licoup-restore-staging")
            }));
            assert_eq!(fs::read(&archive).unwrap(), archived);
            assert_eq!(
                fs::read(source.join("revision/content.json")).unwrap(),
                b"original"
            );
        }
    }
}

#[test]
#[cfg(unix)]
fn prepared_links_are_refused_without_touching_the_referent() {
    let (root, _, archive, target) = fixture("prepared-link", "zip");
    let outside = root.0.join("canary");
    fs::write(&outside, b"unrelated synthetic content").unwrap();
    freeze(&outside);
    let error = restore_data_root_with_preparation(
        &RestoreRequest {
            archive_path: archive,
            target_root: target.clone(),
        },
        |staged, _, _| {
            std::os::unix::fs::symlink(&outside, staged.join("link"))?;
            Ok(())
        },
    )
    .unwrap_err();
    assert!(!error.to_string().is_empty());
    assert!(!target.exists());
    assert_eq!(fs::read(&outside).unwrap(), b"unrelated synthetic content");
    assert!(fs::metadata(&outside).unwrap().permissions().readonly());
}
