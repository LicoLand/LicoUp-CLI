use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::super::io::{load_and_validate_fixture, materialize_semantic_documents};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/contracts/client/fixtures/semantic-conversation/complete-layers.json")
}

#[test]
fn fixture_io_validates_and_materializes_both_semantic_documents() {
    let semantic = load_and_validate_fixture(&fixture_path()).expect("valid fixture");
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let output_dir = std::env::temp_dir().join(format!(
        "lico-conversation-semantic-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&output_dir).expect("create output directory");

    let (json_path, markdown_path, digest) =
        materialize_semantic_documents(&output_dir, &semantic).expect("materialize documents");
    assert_eq!(json_path.file_name().unwrap(), "semantic.json");
    assert_eq!(markdown_path.file_name().unwrap(), "semantic.md");
    assert!(json_path.is_file());
    assert!(markdown_path.is_file());
    assert!(!digest.is_empty());

    fs::remove_dir_all(output_dir).expect("remove output directory");
}
