use super::super::append_guard::{
    append_guard, append_guard_matches, content_guard_digest, content_guard_state,
    extend_content_guard,
};
use super::super::models::{CachedFile, ParserState};
use super::support::temp_dir;
use std::fs;
use std::io::Write;

#[test]
fn append_guard_extends_for_suffixes_and_rejects_prefix_rewrites() {
    let root = temp_dir("append-guard");
    let path = root.join("rollout.jsonl");
    fs::write(&path, b"first\n").unwrap();
    let original_size = fs::metadata(&path).unwrap().len();
    let digest = append_guard(&path, original_size).unwrap();
    let cached = CachedFile {
        modified_ns: 0,
        size: original_size,
        file_id: None,
        parsed_bytes: original_size,
        append_guard: digest,
        state: ParserState::default(),
    };
    assert!(append_guard_matches(&path, &cached));

    let mut state = content_guard_state(&path, original_size).unwrap();
    writeln!(
        fs::OpenOptions::new().append(true).open(&path).unwrap(),
        "second"
    )
    .unwrap();
    let appended_size = fs::metadata(&path).unwrap().len();
    extend_content_guard(&path, original_size, appended_size, &mut state).unwrap();
    assert_eq!(
        content_guard_digest(&state),
        append_guard(&path, appended_size).unwrap()
    );

    fs::write(&path, b"other\nsecond\n").unwrap();
    assert!(!append_guard_matches(&path, &cached));
}
