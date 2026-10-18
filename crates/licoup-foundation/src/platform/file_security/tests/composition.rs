use anyhow::Result;
use std::fs;
use std::path::Path;

#[test]
fn facade_exposes_only_stable_file_security_operations() {
    let _: fn(&Path, &str) -> Result<()> = super::super::append_private_line;
    let _: fn(&Path, &str) -> Result<()> = super::super::atomic_write_private_text;
    let _: fn(&Path, &str, usize) -> Result<()> = super::super::atomic_write_private_text_bounded;
    let _: fn(&Path, &[u8]) -> Result<()> = super::super::create_private_state_marker;
    let _: fn(&Path) -> Result<()> = super::super::ensure_private_dir;
    let _: fn(&Path) -> Result<()> = super::super::harden_private_path;
    let _: fn(&Path) -> Result<()> = super::super::harden_private_tree;
    let _: fn(&Path) -> Result<fs::File> = super::super::open_private_lock_file;
    let _: fn(&Path) -> Result<bool> = super::super::private_state_marker_exists;
    let _: fn(&Path) -> Result<Option<Vec<u8>>> = super::super::read_private_state_marker;
    let _: fn(&Path, usize) -> Result<Option<String>> = super::super::read_private_text_bounded;
    let _: fn(&Path) -> Result<bool> = super::super::remove_private_state_marker;
}
