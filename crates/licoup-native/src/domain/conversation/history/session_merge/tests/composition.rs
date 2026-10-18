use serde_json::json;

use super::super::super::{HistoryPageConfig, HistoryScanConfig};
use super::super::composition::finalize_history_sessions;

#[test]
fn composition_keeps_only_matching_user_authored_sessions() {
    let sessions = vec![
        json!({
            "adapterId": "pi", "nativeSessionId": "keep", "sourcePath": "keep",
            "messages": [{"role": "user", "text": "Keep", "createdAt": 1}],
            "messageCount": 1
        }),
        json!({
            "adapterId": "pi", "nativeSessionId": "drop", "sourcePath": "drop",
            "messages": [{"role": "assistant", "text": "No user", "createdAt": 2}],
            "messageCount": 1
        }),
    ];
    let scan_config = HistoryScanConfig {
        archive_mode: false,
        session_ids: Vec::new(),
        match_terms: Vec::new(),
        match_project_paths: Vec::new(),
        page: HistoryPageConfig {
            offset: 0,
            limit: None,
        },
    };
    let finalized = finalize_history_sessions(sessions, &scan_config);
    assert_eq!(finalized.len(), 1);
    assert_eq!(finalized[0]["nativeSessionId"], "keep");
}

#[test]
fn composition_keeps_agent_owned_empty_copilot_conversations() {
    let sessions = vec![json!({
        "adapterId": "copilot",
        "nativeSessionId": "copilot-empty",
        "messages": []
    })];
    let scan_config = HistoryScanConfig {
        archive_mode: false,
        session_ids: Vec::new(),
        match_terms: Vec::new(),
        match_project_paths: Vec::new(),
        page: HistoryPageConfig {
            offset: 0,
            limit: None,
        },
    };

    let finalized = finalize_history_sessions(sessions, &scan_config);
    assert_eq!(finalized.len(), 1);
    assert_eq!(finalized[0]["nativeSessionId"], "copilot-empty");
}
