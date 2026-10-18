use std::fs;

use super::support::temp_path;

#[test]
fn hardening_a_missing_path_is_a_noop() {
    let path = temp_path("missing-harden");
    super::super::harden_private_path(&path).unwrap();
    assert!(!path.exists());
}

#[cfg(unix)]
#[test]
fn private_tree_rejects_nested_and_broken_symbolic_links() {
    use std::os::unix::fs::symlink;

    let root = temp_path("tree-symlink");
    let nested = root.join("nested");
    fs::create_dir_all(&nested).unwrap();
    let outside = temp_path("tree-outside");
    fs::write(&outside, b"preserve").unwrap();
    symlink(&outside, nested.join("external-link")).unwrap();

    assert!(super::super::harden_private_tree(&root).is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"preserve");
    fs::remove_file(nested.join("external-link")).unwrap();
    symlink(root.join("missing"), nested.join("broken-link")).unwrap();
    assert!(super::super::harden_private_tree(&root).is_err());

    fs::remove_file(nested.join("broken-link")).unwrap();
    fs::remove_dir_all(root).unwrap();
    fs::remove_file(outside).unwrap();
}
