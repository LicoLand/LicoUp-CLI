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
    // Adapters with an a4aa store card decode through the ACO codec that
    // owns the persisted format; cardless adapters keep native parsers.
    if let Some(codec) = super::history::acop_codec_for(adapter) {
        if !adapter.accepts_file(
            path,
            &path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase(),
        ) {
            return Vec::new();
        }
        return super::history::acop_sessions_from_store(
            codec,
            adapter,
            path,
            source_kind,
            metadata,
            &scan_config,
        );
    }
    let Some(parser) = parser_kind(adapter, path) else {
        return Vec::new();
    };
    parse_history(parser, adapter, path, source_kind, metadata, scan_config)
}

fn parser_kind(adapter: HistoryAdapter, path: &Path) -> Option<HistoryParserKind> {
    // Only cardless adapters reach this point: every adapter backed by an
    // a4aa store card was decoded by its ACO codec in `parse_history_file`.
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !adapter.accepts_file(path, &extension) {
        return None;
    }
    match extension.as_str() {
        "jsonl" | "ndjson" => Some(match adapter {
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

    #[test]
    fn cardless_adapters_dispatch_to_native_parsers() {
        assert_eq!(
            parser_kind(HistoryAdapter::LicoAgent, Path::new("session.jsonl")),
            Some(HistoryParserKind::LicoAgentJsonLines)
        );
        assert_eq!(
            parser_kind(HistoryAdapter::CodeBuddy, Path::new("session.md")),
            Some(HistoryParserKind::TextTranscript)
        );
        assert_eq!(
            parser_kind(HistoryAdapter::WorkBuddy, Path::new("workbuddy.db")),
            Some(HistoryParserKind::SqliteDatabase)
        );
        assert_eq!(
            parser_kind(HistoryAdapter::TraeWork, Path::new("session.json")),
            Some(HistoryParserKind::JsonDocument)
        );
        assert_eq!(
            parser_kind(HistoryAdapter::Code, Path::new("transcript.jsonl")),
            Some(HistoryParserKind::GenericJsonLines)
        );
    }

    #[test]
    fn carded_adapters_route_through_the_acop_codec() {
        for adapter in [
            HistoryAdapter::Antigravity,
            HistoryAdapter::ClaudeCode,
            HistoryAdapter::Codex,
            HistoryAdapter::Copilot,
            HistoryAdapter::Cursor,
            HistoryAdapter::DeepSeekHarness,
            HistoryAdapter::Hermes,
            HistoryAdapter::KiloCode,
            HistoryAdapter::KimiCode,
            HistoryAdapter::OpenClaw,
            HistoryAdapter::OpenCode,
            HistoryAdapter::Pi,
        ] {
            assert!(
                super::super::history::acop_codec_for(adapter).is_some(),
                "{} keeps an ACOP codec",
                adapter.id()
            );
        }
        for adapter in [
            HistoryAdapter::Code,
            HistoryAdapter::LicoAgent,
            HistoryAdapter::WorkBuddy,
            HistoryAdapter::CodeBuddy,
            HistoryAdapter::TraeWork,
            HistoryAdapter::TraeAgent,
        ] {
            assert!(
                super::super::history::acop_codec_for(adapter).is_none(),
                "{} stays on native parsers",
                adapter.id()
            );
        }
    }

    #[test]
    fn rejects_files_outside_the_selected_adapter_contract() {
        use super::super::source_catalog::{adapter_for_agent, usage_adapter_for_agent};
        assert_eq!(
            adapter_for_agent("deepseek-harness"),
            Some(HistoryAdapter::DeepSeekHarness)
        );
        assert_eq!(
            usage_adapter_for_agent("deepseek-harness"),
            Some(HistoryAdapter::DeepSeekHarness)
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
            parser_kind(HistoryAdapter::LicoAgent, Path::new("session.md")),
            None
        );
    }
}
