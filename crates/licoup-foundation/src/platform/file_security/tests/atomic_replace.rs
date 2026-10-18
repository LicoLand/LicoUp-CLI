use std::fs;

use super::support::temp_path;

#[test]
fn bounded_atomic_write_rejects_content_before_creating_state() {
    let path = temp_path("atomic-bound").join("state.json");

    assert!(super::super::atomic_write_private_text_bounded(&path, "oversized", 3).is_err());
    assert!(!path.exists());
}

#[test]
fn non_cross_device_rename_errors_do_not_enter_the_copy_fallback() {
    let root = temp_path("atomic-non-exdev");
    fs::create_dir_all(&root).unwrap();
    let temporary = root.join("source.tmp");
    let destination = root.join("destination");
    fs::write(&temporary, b"replacement").unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("sentinel"), b"preserve").unwrap();

    let result = super::super::atomic_replace::rename_into_place(&temporary, &destination);

    assert!(result.is_err());
    assert_eq!(fs::read(destination.join("sentinel")).unwrap(), b"preserve");
    assert_eq!(fs::read(&temporary).unwrap(), b"replacement");
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn atomic_replace_rejects_a_destination_symlink_without_touching_its_referent() {
    use std::os::unix::fs::symlink;

    let root = temp_path("atomic-destination-link");
    fs::create_dir_all(&root).unwrap();
    let temporary = root.join("source.tmp");
    let referent = root.join("referent");
    let destination = root.join("destination");
    fs::write(&temporary, b"replacement").unwrap();
    fs::write(&referent, b"preserve").unwrap();
    symlink(&referent, &destination).unwrap();

    let result = super::super::atomic_replace::rename_into_place(&temporary, &destination);

    assert!(result.is_err());
    assert_eq!(fs::read(&referent).unwrap(), b"preserve");
    assert_eq!(fs::read(&temporary).unwrap(), b"replacement");
    fs::remove_dir_all(root).unwrap();
}
