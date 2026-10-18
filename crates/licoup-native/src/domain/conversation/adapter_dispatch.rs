//! Single routing authority from an agent history source to its parser.

use super::parser_port::{HistoryParserKind, HistoryScanConfig, parse_history};
use super::source_catalog::HistoryAdapter;
use serde_json::Value;
use std::fs;
use std::path::Path;

pub(crate) fn parse_history_file(
    adapter: HistoryAdapter,
    path: &Path,
    source_kind: &str,
    metadata: &fs::Metadata,
    scan_config: HistoryScanConfig,
) -> Vec<Value> {
    let Some(parser) = parser_kind(adapter, path) else {
        return Vec::new();
    };
    parse_history(parser, adapter, path, source_kind, metadata, scan_config)
}

fn parser_kind(adapter: HistoryAdapter, path: &Path) -> Option<HistoryParserKind> {
    // Harness historical usage has a separate exact metadata reader. Its
    // compressed event log must never enter a generic transcript parser.
    if adapter == HistoryAdapter::DeepSeekHarness {
        return None;
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !adapter.accepts_file(path, &extension) {
        return None;
    }
    if adapter == HistoryAdapter::KimiCode {
        return Some(HistoryParserKind::KimiCodeWire);
    }
    match extension.as_str() {
        "jsonl" | "ndjson" => Some(match adapter {
            HistoryAdapter::Codex => HistoryParserKind::CodexRollout,
            HistoryAdapter::Copilot => HistoryParserKind::CopilotTranscript,
            HistoryAdapter::Pi => HistoryParserKind::PiJsonLines,
            HistoryAdapter::LicoAgent => HistoryParserKind::LicoAgentJsonLines,
            _ => HistoryParserKind::GenericJsonLines,
        }),
        "json" => Some(HistoryParserKind::JsonDocument),
        "md" | "markdown" | "txt" | "log" => Some(HistoryParserKind::TextTranscript),
        "sqlite" | "sqlite3" | "db" | "vscdb" => Some(HistoryParserKind::SqliteDatabase),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn dispatches_agent_specific_and_generic_formats() {
        let kimi_wire = PathBuf::from("workspace")
            .join("agents")
            .join("worker")
            .join("wire.jsonl");
        assert_eq!(
            parser_kind(HistoryAdapter::KimiCode, &kimi_wire),
            Some(HistoryParserKind::KimiCodeWire)
        );
        assert_eq!(
            parser_kind(HistoryAdapter::Codex, Path::new("rollout.jsonl")),
            Some(HistoryParserKind::CodexRollout)
        );
        assert_eq!(
            parser_kind(HistoryAdapter::OpenCode, Path::new("session.md")),
            Some(HistoryParserKind::TextTranscript)
        );
        assert_eq!(
            parser_kind(HistoryAdapter::Cursor, Path::new("state.vscdb")),
            Some(HistoryParserKind::SqliteDatabase)
        );
        assert_eq!(
            parser_kind(HistoryAdapter::LicoAgent, Path::new("session.jsonl")),
            Some(HistoryParserKind::LicoAgentJsonLines)
        );
    }

    #[test]
    fn rejects_files_outside_the_selected_adapter_contract() {
        use super::super::source_catalog::{adapter_for_agent, usage_adapter_for_agent};
        assert_eq!(adapter_for_agent("deepseek-harness"), None);
        assert_eq!(
            usage_adapter_for_agent("deepseek-harness"),
            Some(HistoryAdapter::DeepSeekHarness)
        );
        assert_eq!(
            parser_kind(HistoryAdapter::DeepSeekHarness, Path::new("session.jsonl")),
            None
        );
        assert_eq!(
            parser_kind(HistoryAdapter::Cursor, Path::new("notes.md")),
            None
        );
        assert_eq!(
            parser_kind(HistoryAdapter::Codex, Path::new("rollout.jsonl.backup")),
            None
        );
        assert_eq!(
            parser_kind(HistoryAdapter::KimiCode, Path::new("wire.jsonl")),
            None
        );
        assert_eq!(
            parser_kind(HistoryAdapter::LicoAgent, Path::new("session.md")),
            None
        );
    }
}
