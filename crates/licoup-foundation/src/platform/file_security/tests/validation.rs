use std::fs;
use std::path::Path;

use super::support::temp_path;

#[test]
fn parent_traversal_is_rejected_before_path_resolution() {
    assert!(super::super::validate_no_symlink_ancestors(Path::new("state/../escape")).is_err());
}

#[cfg(windows)]
#[test]
fn an_absolute_drive_path_does_not_probe_its_prefix_as_a_drive_relative_path() {
    let path = temp_path("absolute-drive-prefix");
    assert!(path.is_absolute());
    super::super::validate_no_symlink_ancestors(&path).unwrap();
}

#[test]
fn a_directory_is_not_an_atomic_regular_file() {
    let root = temp_path("regular-file-validation");
    fs::create_dir_all(&root).unwrap();

    assert!(
        super::super::validation::validate_regular_file_or_missing_no_follow(&root, false).is_err()
    );

    fs::remove_dir(root).unwrap();
}

#[cfg(unix)]
#[test]
fn export_validation_rejects_a_symbolic_link_destination() {
    use std::os::unix::fs::symlink;

    let root = temp_path("export-link");
    fs::create_dir_all(&root).unwrap();
    let referent = root.join("referent");
    let destination = root.join("destination");
    fs::write(&referent, b"preserve").unwrap();
    symlink(&referent, &destination).unwrap();

    assert!(super::super::validate_export_destination(&destination).is_err());
    assert_eq!(fs::read(referent).unwrap(), b"preserve");

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn distinct_open_nodes_fail_the_same_file_check() {
    let root = temp_path("same-file");
    fs::create_dir_all(&root).unwrap();
    let left = root.join("left");
    let right = root.join("right");
    fs::write(&left, b"same").unwrap();
    fs::write(&right, b"same").unwrap();

    assert!(
        super::super::validation::ensure_same_file(
            &fs::metadata(&left).unwrap(),
            &fs::metadata(&right).unwrap()
        )
        .is_err()
    );

    fs::remove_dir_all(root).unwrap();
}
