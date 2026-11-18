use std::fs;
use std::os::unix::fs::PermissionsExt;

use super::support::temp_path;

#[test]
fn atomic_write_and_tree_hardening_apply_owner_only_modes() {
    let root = temp_path("unix-modes");
    let path = root.join("state.json");

    super::super::atomic_write_private_text(&path, "{\"ok\":true}\n").unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    super::super::harden_private_tree(&root).unwrap();
    assert_eq!(
        fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unix_stat_validation_rejects_a_world_readable_private_file() {
    use nix::sys::stat::fstat;
    use std::os::fd::AsRawFd;

    let path = temp_path("unix-stat-mode");
    fs::write(&path, b"state").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let file = fs::File::open(&path).unwrap();

    assert!(
        super::super::unix_hardening::validate_private_file_stat(&fstat(file.as_raw_fd()).unwrap())
            .is_err()
    );

    drop(file);
    fs::remove_file(path).unwrap();
}
