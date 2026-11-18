use super::super::file_collection::{collect_usage_files, file_metadata};
use super::support::temp_dir;
use std::fs;

#[test]
fn iterative_collection_deduplicates_roots_and_keeps_only_usage_streams() {
    let root = temp_dir("file-collection");
    let nested = root.join("nested");
    fs::create_dir_all(&nested).unwrap();
    let jsonl = nested.join("first.jsonl");
    let ndjson = nested.join("second.NDJSON");
    fs::write(&jsonl, "{}\n").unwrap();
    fs::write(&ndjson, "{}\n").unwrap();
    fs::write(nested.join("ignored.json"), "{}").unwrap();

    let files = collect_usage_files(&[root.clone(), root]);
    assert_eq!(files.len(), 2);
    assert!(files.contains(&jsonl));
    assert!(files.contains(&ndjson));
    let metadata = file_metadata(&jsonl).unwrap();
    assert_eq!(metadata.size, 3);
}
