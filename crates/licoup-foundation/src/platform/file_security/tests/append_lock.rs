use std::fs;
use std::sync::{Arc, Barrier};
use std::thread;

use super::support::temp_path;

#[test]
fn append_rejects_a_record_above_the_independent_line_limit() {
    let path = temp_path("append-line-bound").join("activity.jsonl");
    let oversized = "x".repeat(super::super::policy::PRIVATE_APPEND_LINE_MAX_BYTES + 1);

    assert!(super::super::append_private_line(&path, &oversized).is_err());
    assert!(!path.exists());
}

#[test]
fn lock_file_contains_only_the_fixed_private_marker() {
    let root = temp_path("lock-marker");
    let path = root.join("operation.lock");

    let lock = super::super::open_private_lock_file(&path).unwrap();
    assert_eq!(
        super::super::read_private_state_marker(&path).unwrap(),
        Some(super::super::policy::PRIVATE_LOCK_MARKER.to_vec())
    );
    drop(lock);

    fs::remove_file(path).unwrap();
    fs::remove_dir(root).unwrap();
}

#[test]
fn concurrent_first_open_initializes_one_complete_private_lock_marker() {
    let root = temp_path("lock-marker-concurrent");
    let path = root.join("operation.lock");
    let barrier = Arc::new(Barrier::new(9));
    let handles = (0..8)
        .map(|_| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                super::super::open_private_lock_file(&path)
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    for handle in handles {
        drop(handle.join().unwrap().unwrap());
    }
    assert_eq!(
        super::super::read_private_state_marker(&path).unwrap(),
        Some(super::super::policy::PRIVATE_LOCK_MARKER.to_vec())
    );

    fs::remove_file(path).unwrap();
    fs::remove_dir(root).unwrap();
}

#[cfg(unix)]
#[test]
fn append_creates_an_owner_only_regular_jsonl_file() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_path("append-owner-only");
    let path = root.join("activity.jsonl");

    super::super::append_private_line(&path, r#"{"ok":true}"#).unwrap();

    let metadata = fs::symlink_metadata(&path).unwrap();
    assert!(metadata.file_type().is_file());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    assert_eq!(fs::read_to_string(&path).unwrap(), "{\"ok\":true}\n");

    fs::remove_file(path).unwrap();
    fs::remove_dir(root).unwrap();
}

#[cfg(unix)]
#[test]
fn append_rejects_a_symbolic_link_without_touching_its_referent() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = temp_path("append-symlink");
    fs::create_dir_all(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let referent = root.join("referent.jsonl");
    let append_path = root.join("activity.jsonl");
    fs::write(&referent, b"preserve\n").unwrap();
    fs::set_permissions(&referent, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&referent, &append_path).unwrap();

    assert!(super::super::append_private_line(&append_path, "blocked").is_err());
    assert_eq!(fs::read(&referent).unwrap(), b"preserve\n");

    fs::remove_file(append_path).unwrap();
    fs::remove_file(referent).unwrap();
    fs::remove_dir(root).unwrap();
}

#[cfg(unix)]
#[test]
fn append_rejects_a_user_owned_symbolic_link_ancestor() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = temp_path("append-ancestor");
    let outside = temp_path("append-ancestor-outside");
    let outside_nested = outside.join("nested");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside_nested).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&outside_nested, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&outside, root.join("redirect")).unwrap();
    let append_path = root.join("redirect/nested/activity.jsonl");

    assert!(super::super::append_private_line(&append_path, "blocked").is_err());
    assert!(!outside_nested.join("activity.jsonl").exists());

    fs::remove_file(root.join("redirect")).unwrap();
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(outside).unwrap();
}
