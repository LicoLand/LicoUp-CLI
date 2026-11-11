use super::super::clock::timestamp;
use super::super::request::display_path;
use super::super::{create, preview};
use crate::platform::client_state::ClientStateStore;
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn archive_job_fixture(name: &str, content: &str) -> (PathBuf, PathBuf, PathBuf) {
    let state = temp_dir(&format!("{}-state", name));
    let home = temp_dir(&format!("{}-home", name));
    let archive_root = temp_dir(&format!("{}-archive", name));
    let history = temp_dir(&format!("{}-history", name));
    fs::write(
        history.join("history.jsonl"),
        format!(
            r#"{{"sessionId":"{}","role":"user","content":"{}"}}"#,
            name, content
        ),
    )
    .unwrap();
    let store = ClientStateStore::new(state.clone()).unwrap();
    store
        .write_collection(
            "targets",
            json!({
                "items": [{
                    "target": "codex",
                    "manual": true,
                    "historyRoots": [display_path(&history)]
                }]
            }),
        )
        .unwrap();
    (state, home, archive_root)
}

pub(super) fn create_planned(mut params: Value) -> anyhow::Result<Value> {
    let plan = preview(&params)?;
    let binding = plan["plan"]["binding"].as_str().unwrap_or_default();
    params
        .as_object_mut()
        .expect("archive test params must be an object")
        .insert("planBinding".to_string(), json!(binding));
    create(&params)
}

pub(super) fn corrupt_first_raw_content(archive_root: &Path, folder: &str) {
    let index_path = archive_root.join(folder).join("conversation-index.jsonl");
    let raw = fs::read_to_string(&index_path).unwrap();
    let first = raw.lines().next().unwrap();
    let record: Value = serde_json::from_str(first).unwrap();
    let raw_path = PathBuf::from(record["raw_content_path"].as_str().unwrap());
    fs::write(raw_path, b"{\"corrupt\":true}\n").unwrap();
}

pub(super) fn temp_dir(name: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!(
        "conversation-archive-jobs-{}-{}",
        name,
        timestamp()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}
