use std::fs;

use super::support::temp_path;

#[test]
fn marker_round_trip_and_durable_removal_are_independent() {
    let root = temp_path("marker-round-trip");
    let path = root.join("security.guard");
    let body = br#"{"schemaVersion":1,"state":"blocked"}"#;

    super::super::create_private_state_marker(&path, body).unwrap();
    assert_eq!(
        super::super::read_private_state_marker(&path).unwrap(),
        Some(body.to_vec())
    );
    assert!(super::super::private_state_marker_exists(&path).unwrap());
    assert!(super::super::remove_private_state_marker(&path).unwrap());
    assert!(!super::super::private_state_marker_exists(&path).unwrap());

    fs::remove_dir(root).unwrap();
}

#[test]
fn bounded_text_reader_rejects_oversized_state() {
    let root = temp_path("bounded-reader");
    let path = root.join("state.txt");
    super::super::create_private_state_marker(&path, b"bounded").unwrap();

    assert!(super::super::read_private_text_bounded(&path, 3).is_err());

    fs::remove_file(path).unwrap();
    fs::remove_dir(root).unwrap();
}

#[cfg(unix)]
#[test]
fn marker_rejects_symbolic_link_substitution() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = temp_path("marker-symlink");
    fs::create_dir_all(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let target = root.join("target");
    let marker = root.join("security.guard");
    fs::write(&target, b"insecure").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&target, &marker).unwrap();

    assert!(super::super::private_state_marker_exists(&marker).is_err());
    assert!(super::super::remove_private_state_marker(&marker).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"insecure");

    fs::remove_file(marker).unwrap();
    fs::remove_file(target).unwrap();
    fs::remove_dir(root).unwrap();
}
