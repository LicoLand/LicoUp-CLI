#![cfg(windows)]

use std::fs;
use std::process::Command;

use super::support::temp_path;

#[test]
fn private_path_hardening_applies_owner_rights_acl() {
    let path = temp_path("owner-rights.txt");
    fs::write(&path, "private").unwrap();

    super::super::harden_private_path(&path).unwrap();

    let output = Command::new("icacls").arg(&path).output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("OWNER RIGHTS:(F)"));

    fs::remove_file(path).unwrap();
}
