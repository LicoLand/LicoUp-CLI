use std::fs::{self, OpenOptions};
use std::io::Write;

use super::support::temp_path;

#[test]
fn file_and_directory_sync_have_a_bounded_independent_closure() {
    let root = temp_path("sync");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("state");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .unwrap();
    file.write_all(b"state").unwrap();

    super::super::sync::file(&mut file).unwrap();
    super::super::sync::parent(&path).unwrap();
    super::super::sync::directory(&root).unwrap();

    drop(file);
    fs::remove_file(path).unwrap();
    fs::remove_dir(root).unwrap();
}
