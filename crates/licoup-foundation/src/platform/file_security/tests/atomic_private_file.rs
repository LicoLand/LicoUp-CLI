use std::fs;
use std::io::Write;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use super::support::temp_path;

#[test]
fn dropping_an_uncommitted_private_file_leaves_no_output() {
    let root = temp_path("atomic-private-drop");
    fs::create_dir_all(&root).unwrap();
    let destination = root.join("archive.bin");
    {
        let mut writer = super::super::AtomicPrivateFile::create(&destination).unwrap();
        writer.file_mut().write_all(b"partial").unwrap();
    }
    assert!(!destination.exists());
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn committing_a_private_file_replaces_the_destination_privately() {
    let root = temp_path("atomic-private-commit");
    fs::create_dir_all(&root).unwrap();
    let destination = root.join("archive.bin");
    let mut writer = super::super::AtomicPrivateFile::create(&destination).unwrap();
    writer.file_mut().write_all(b"archive").unwrap();
    writer.commit().unwrap();

    assert_eq!(fs::read(&destination).unwrap(), b"archive");
    #[cfg(unix)]
    assert_eq!(
        fs::symlink_metadata(&destination)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn a_destination_symlink_is_refused_and_its_referent_is_preserved() {
    use std::os::unix::fs::symlink;

    let root = temp_path("atomic-private-link");
    fs::create_dir_all(&root).unwrap();
    let referent = root.join("referent");
    let destination = root.join("destination");
    fs::write(&referent, b"preserve").unwrap();
    symlink(&referent, &destination).unwrap();

    let result = super::super::AtomicPrivateFile::create(&destination);
    assert!(result.is_err());
    assert_eq!(fs::read(&referent).unwrap(), b"preserve");
    assert!(fs::symlink_metadata(&destination).unwrap().is_symlink());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn replacing_a_hard_link_alias_preserves_the_other_name() {
    let root = temp_path("atomic-private-hardlink");
    fs::create_dir_all(&root).unwrap();
    let referent = root.join("referent");
    let destination = root.join("alias");
    fs::write(&referent, b"original").unwrap();
    fs::hard_link(&referent, &destination).unwrap();

    let mut writer = super::super::AtomicPrivateFile::create(&destination).unwrap();
    writer.file_mut().write_all(b"archive").unwrap();
    writer.commit().unwrap();

    assert_eq!(fs::read(&referent).unwrap(), b"original");
    assert_eq!(fs::read(&destination).unwrap(), b"archive");
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn a_pre_replacement_sync_failure_keeps_the_previous_output() {
    let root = temp_path("atomic-pre-sync-failure");
    fs::create_dir_all(&root).unwrap();
    let temporary = root.join("temporary.tmp");
    let destination = root.join("destination");
    fs::write(&temporary, b"replacement").unwrap();
    // The published file must satisfy the private-file policy before the sync runs.
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&destination, b"previous").unwrap();

    let error = super::super::atomic_replace::commit_with_sync(&temporary, &destination, |_| {
        Err(anyhow::anyhow!("injected pre-replacement sync failure"))
    })
    .expect_err("a failed pre-replacement sync must fail the commit");

    assert_eq!(error.to_string(), "injected pre-replacement sync failure");
    assert_eq!(fs::read(&destination).unwrap(), b"previous");
    assert!(
        temporary.exists(),
        "temporary cleanup belongs to the caller's checked abort"
    );
    fs::remove_file(&temporary).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn a_post_replacement_sync_failure_reports_unconfirmed_and_keeps_the_replacement() {
    let root = temp_path("atomic-post-sync-failure");
    fs::create_dir_all(&root).unwrap();
    let temporary = root.join("temporary.tmp");
    let destination = root.join("destination");
    fs::write(&temporary, b"replacement").unwrap();
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&destination, b"previous").unwrap();

    let mut calls = 0_u32;
    let durability =
        super::super::atomic_replace::commit_with_sync(&temporary, &destination, |_| {
            calls += 1;
            if calls == 2 {
                Err(anyhow::anyhow!("injected post-replacement sync failure"))
            } else {
                Ok(())
            }
        })
        .expect("the replacement completes");

    assert_eq!(durability, super::super::CommitDurability::Unconfirmed);
    assert_eq!(fs::read(&destination).unwrap(), b"replacement");
    assert!(!temporary.exists());
    assert_eq!(
        fs::read_dir(&root).unwrap().count(),
        1,
        "only the confirmed replacement remains"
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn a_pre_replacement_sync_failure_leaves_a_previously_absent_destination_absent() {
    let root = temp_path("atomic-pre-sync-absent");
    fs::create_dir_all(&root).unwrap();
    let temporary = root.join("temporary.tmp");
    let destination = root.join("destination");
    fs::write(&temporary, b"replacement").unwrap();
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)).unwrap();

    let error = super::super::atomic_replace::commit_with_sync(&temporary, &destination, |_| {
        Err(anyhow::anyhow!("injected pre-replacement sync failure"))
    })
    .expect_err("a failed pre-replacement sync must fail the commit");

    assert_eq!(error.to_string(), "injected pre-replacement sync failure");
    assert!(!destination.exists());
    assert!(temporary.exists());
    fs::remove_file(&temporary).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn a_post_replacement_sync_failure_publishes_into_a_previously_absent_destination() {
    let root = temp_path("atomic-post-sync-absent");
    fs::create_dir_all(&root).unwrap();
    let temporary = root.join("temporary.tmp");
    let destination = root.join("destination");
    fs::write(&temporary, b"replacement").unwrap();
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)).unwrap();

    let mut calls = 0_u32;
    let durability =
        super::super::atomic_replace::commit_with_sync(&temporary, &destination, |_| {
            calls += 1;
            if calls == 2 {
                Err(anyhow::anyhow!("injected post-replacement sync failure"))
            } else {
                Ok(())
            }
        })
        .expect("the replacement completes");

    assert_eq!(durability, super::super::CommitDurability::Unconfirmed);
    assert_eq!(fs::read(&destination).unwrap(), b"replacement");
    assert!(!temporary.exists());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn a_failed_commit_reports_a_retained_temporary_when_cleanup_cannot_run() {
    let root = temp_path("atomic-retained-temporary");
    fs::create_dir_all(&root).unwrap();
    let destination = root.join("archive.bin");
    let mut writer = super::super::AtomicPrivateFile::create(&destination).unwrap();
    writer.file_mut().write_all(b"archive").unwrap();
    // The destination cannot be replaced and the temporary cannot be removed.
    fs::set_permissions(&root, fs::Permissions::from_mode(0o500)).unwrap();

    let failure = writer.commit().expect_err("the replacement must fail");
    let (error, retained) = failure.into_parts();
    assert!(
        error.to_string().contains("could not be committed"),
        "{error}"
    );
    assert!(
        matches!(retained, super::super::CleanupOutcome::Retained(_)),
        "the retained temporary is named"
    );
    assert!(!destination.exists());

    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn discarding_reports_a_temporary_that_cannot_be_removed() {
    let root = temp_path("atomic-retained-discard");
    fs::create_dir_all(&root).unwrap();
    let destination = root.join("archive.bin");
    let writer = super::super::AtomicPrivateFile::create(&destination).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o500)).unwrap();

    let outcome = writer.discard();
    assert!(
        matches!(outcome, super::super::CleanupOutcome::Retained(_)),
        "{outcome:?}"
    );

    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_bare_relative_output_path_commits() {
    let relative =
        std::path::PathBuf::from(format!("lico-relative-output-{}.bin", std::process::id()));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(relative.clone());

    let mut writer = super::super::AtomicPrivateFile::create(&relative).unwrap();
    writer.file_mut().write_all(b"relative").unwrap();
    writer.commit().unwrap();

    assert_eq!(fs::read(&relative).unwrap(), b"relative");
}

#[test]
fn removed_temporary_sync_failure_is_not_reported_as_retained_or_durable() {
    let root = temp_path("atomic-cleanup-sync");
    fs::create_dir_all(&root).unwrap();
    let temp = root.join("temporary");
    fs::write(&temp, b"temporary").unwrap();
    let outcome = super::super::atomic_replace::remove_with_sync(&temp, |path| {
        assert_eq!(path, temp);
        assert!(!path.exists(), "sync follows removal");
        Err(anyhow::anyhow!("injected cleanup sync fault"))
    });
    assert_eq!(
        outcome,
        super::super::CleanupOutcome::DurabilityUnconfirmed(root.clone())
    );
    assert!(!temp.exists());
    fs::remove_dir_all(root).unwrap();
}
