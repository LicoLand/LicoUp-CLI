//! Narrow parser execution port used by history-source dispatch.

pub(crate) use super::history::HistoryScanConfig;
use super::history::{
    parse_json_sessions, parse_jsonl_sessions, parse_lico_agent_session, parse_sqlite_sessions,
    parse_text_session,
};
use super::source_catalog::HistoryAdapter;
use serde_json::Value;
use std::fs;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HistoryParserKind {
    LicoAgentJsonLines,
    GenericJsonLines,
    JsonDocument,
    TextTranscript,
    SqliteDatabase,
}

pub(crate) fn parse_history(
    parser: HistoryParserKind,
    adapter: HistoryAdapter,
    path: &Path,
    source_kind: &str,
    metadata: &fs::Metadata,
    scan_config: HistoryScanConfig,
) -> Vec<Value> {
    match parser {
        HistoryParserKind::LicoAgentJsonLines => {
            parse_lico_agent_session(path, source_kind, metadata)
                .map(|session| vec![session])
                .unwrap_or_else(|| {
                    parse_jsonl_sessions(adapter, path, source_kind, metadata, scan_config)
                })
        }
        HistoryParserKind::GenericJsonLines => {
            parse_jsonl_sessions(adapter, path, source_kind, metadata, scan_config)
        }
        HistoryParserKind::JsonDocument => {
            parse_json_sessions(adapter, path, source_kind, metadata, scan_config)
        }
        HistoryParserKind::TextTranscript => {
            parse_text_session(adapter, path, source_kind, metadata)
        }
        HistoryParserKind::SqliteDatabase => {
            parse_sqlite_sessions(adapter, path, source_kind, metadata, scan_config)
        }
    }
}
