use serde_json::Value;

use super::super::HistoryScanConfig;
use super::super::query_filter::history_session_has_user_authored_message;
use super::codex_lineage::merge_codex_rollout_lineage_sessions;
use super::delegated_merge::{merge_delegated_subagent_sessions, session_is_delegated_subagent};

pub(crate) fn finalize_history_sessions(
    sessions: Vec<Value>,
    scan_config: &HistoryScanConfig,
) -> Vec<Value> {
    let sessions = merge_delegated_subagent_sessions(sessions, scan_config.single_session_id());
    let sessions = merge_codex_rollout_lineage_sessions(sessions);
    sessions
        .into_iter()
        .filter(|session| {
            session_is_delegated_subagent(session)
                || history_session_has_user_authored_message(session)
        })
        .filter(|session| scan_config.matches_session(session))
        .map(|session| scan_config.compact_session_for_archive_discovery(session))
        .collect()
}
