use super::*;

fn selection_fixture(label: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let state = temp_dir(&format!("{label}-state"));
    let home = temp_dir(&format!("{label}-home"));
    let destination = temp_dir(&format!("{label}-destination"));
    let history = temp_dir(&format!("{label}-history"));
    fs::write(
        history.join("history.jsonl"),
        [
            r#"{"sessionId":"exact","role":"user","content":"Agent Studio release notes"}"#,
            r#"{"sessionId":"compact","role":"user","content":"agentstudio compact alias"}"#,
            r#"{"sessionId":"other","role":"user","content":"Unrelated local conversation"}"#,
            r#"{"sessionId":"Agent Studio","role":"user","content":"Opaque identity only"}"#,
        ]
        .join("\n"),
    )
    .unwrap();
    ClientStateStore::new(state.clone())
        .unwrap()
        .write_collection(
            TARGETS_COLLECTION,
            json!({
                "items": [{
                    "target": "codex",
                    "manual": true,
                    "historyRoots": [display_path(&history)]
                }]
            }),
        )
        .unwrap();
    (state, home, destination, history)
}

#[test]
fn all_preview_counts_every_real_local_conversation_and_binds_conflict() {
    let (state, home, destination, _) = selection_fixture("all-preview");
    let params = json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "agent": "codex",
        "selectionMode": "all",
        "path": display_path(&destination)
    });

    let preview = archive_selection_preview(&params).unwrap();
    assert_eq!(preview["selectionMode"], "all");
    assert_eq!(preview["query"], "");
    assert_eq!(preview["count"], 4);
    assert_eq!(preview["conflict"], false);

    let collected = archive_selection_collect(&params).unwrap();
    assert_eq!(collected["selectedCount"], 4);
    let next_preview = archive_selection_preview(&params).unwrap();
    assert_eq!(next_preview["conflict"], true);
}

#[test]
fn exact_keyword_preview_does_not_expand_compact_aliases() {
    let (state, home, destination, _) = selection_fixture("exact-preview");
    let preview = archive_selection_preview(&json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "agent": "codex",
        "selectionMode": "exact-keyword",
        "query": "Agent Studio",
        "path": display_path(&destination)
    }))
    .unwrap();

    assert_eq!(preview["selectionMode"], "exact-keyword");
    assert_eq!(preview["query"], "Agent Studio");
    assert_eq!(preview["count"], 1);
}

#[test]
fn global_all_preview_spans_every_explicitly_discovered_agent() {
    let state = temp_dir("global-all-state");
    let home = temp_dir("global-all-home");
    let destination = temp_dir("global-all-destination");
    let codex_history = temp_dir("global-all-codex-history");
    let opencode_history = temp_dir("global-all-opencode-history");
    fs::write(
        codex_history.join("history.jsonl"),
        r#"{"sessionId":"codex-one","role":"user","content":"Codex local history"}"#,
    )
    .unwrap();
    fs::write(
        opencode_history.join("history.jsonl"),
        r#"{"sessionId":"opencode-one","role":"user","content":"OpenCode local history"}"#,
    )
    .unwrap();

    let preview = archive_selection_preview(&json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "selectionMode": "all",
        "path": display_path(&destination),
        "targetScan": {
            "source": "synthetic-local-targets",
            "candidates": [
                {
                    "target": "codex",
                    "status": "manual",
                    "historyRoots": [display_path(&codex_history)]
                },
                {
                    "target": "opencode",
                    "status": "manual",
                    "historyRoots": [display_path(&opencode_history)]
                }
            ]
        }
    }))
    .unwrap();

    assert_eq!(preview["count"], 2);
    assert_eq!(preview["source"]["agents"], json!(["codex", "opencode"]));
}
