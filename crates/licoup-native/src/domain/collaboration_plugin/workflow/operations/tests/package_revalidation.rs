use std::path::PathBuf;

use super::super::super::super::package::SelectedPayloadFile;
use super::super::package_revalidation::planned_payload;

#[test]
fn planned_payload_binds_paths_digests_and_exact_byte_lengths() {
    let file = SelectedPayloadFile {
        selection_id: "server-core".to_owned(),
        source_relative_path: PathBuf::from("payload/server-core/main.json"),
        destination_relative_path: PathBuf::from("config/main.json"),
        digest_sha256: "a".repeat(64),
        bytes: b"local-only".to_vec(),
    };
    let planned = planned_payload(&[file]).unwrap();
    assert_eq!(planned[0].selection_id, "server-core");
    assert_eq!(
        planned[0].source_relative_path,
        "payload/server-core/main.json"
    );
    assert_eq!(planned[0].destination_relative_path, "config/main.json");
    assert_eq!(planned[0].bytes, 10);
}

#[test]
fn planned_payload_rejects_parent_path_components() {
    let file = SelectedPayloadFile {
        selection_id: "server-core".to_owned(),
        source_relative_path: PathBuf::from("../private"),
        destination_relative_path: PathBuf::from("main.json"),
        digest_sha256: "a".repeat(64),
        bytes: Vec::new(),
    };
    assert_eq!(
        planned_payload(&[file]).unwrap_err().to_string(),
        "collaboration_workflow_relative_path_invalid"
    );
}
