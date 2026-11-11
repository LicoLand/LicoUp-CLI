use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticLayer {
    Thread,
    Execution,
    Artifacts,
    Audit,
    Raw,
}

impl SemanticLayer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Thread => "thread",
            Self::Execution => "execution",
            Self::Artifacts => "artifacts",
            Self::Audit => "audit",
            Self::Raw => "raw",
        }
    }
}

pub fn privacy_defaults() -> Value {
    json!({
        "defaultView": "thread",
        "hideRawInDefaultView": true,
        "hideAuditInDefaultView": true,
        "redactPaths": true,
        "redactTokens": true,
        "redactFullCommandPayloads": true
    })
}

pub fn layer_for_history_kind(kind: &str) -> SemanticLayer {
    match kind {
        "tool_call" | "tool-call" | "tool_result" | "tool-result" | "reasoning" | "metadata"
        | "error" | "event" | "terminal" | "plan" | "progress" | "retry" => {
            SemanticLayer::Execution
        }
        "artifact" | "file" | "diff" | "document" => SemanticLayer::Artifacts,
        "audit" => SemanticLayer::Audit,
        "raw" => SemanticLayer::Raw,
        _ => SemanticLayer::Thread,
    }
}

pub fn execution_event_kind(card_type: &str, source_item_type: &str) -> &'static str {
    let semantic = if !source_item_type.trim().is_empty() {
        source_item_type
    } else {
        card_type
    };
    let normalized = normalize_token(semantic);
    if normalized.contains("terminal")
        || normalized.contains("run-command")
        || normalized.contains("shell")
        || normalized.contains("bash")
    {
        return "terminal";
    }
    if normalized.contains("plan") {
        return "plan";
    }
    if normalized.contains("progress") || normalized.contains("status") {
        return "progress";
    }
    if normalized.contains("retry") {
        return "retry";
    }
    if normalized.contains("error") || normalized.contains("failure") {
        return "error";
    }
    if normalized.contains("reasoning") || normalized.contains("thinking") {
        return "reasoning";
    }
    if normalized.contains("tool-result")
        || normalized.contains("tool_result")
        || normalized.contains("function-call-output")
        || normalized.contains("function-result")
    {
        return "tool-result";
    }
    if normalized.contains("tool") || normalized.contains("function") {
        return "tool-call";
    }
    "event"
}

pub fn evidence_kind_from_source(source_kind: &str) -> &'static str {
    match source_kind.trim().to_ascii_lowercase().as_str() {
        "jsonl"
        | "codex-session-store"
        | "codex-archived-session-store"
        | "kimi-code-session-store"
        | "pi-session-store" => "jsonl",
        "json" | "claude-global-state" | "cursor-cli-chats" => "json",
        "sqlite"
        | "sqlite-row"
        | "db"
        | "codex-session-index"
        | "cursor-workspace-storage"
        | "cursor-global-storage" => "sqlite-row",
        "md" | "markdown" | "codex-memory" | "codex-rollout-summary" => "markdown",
        "txt" | "text" => "text",
        _ => "unknown",
    }
}

pub fn hash_text(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn synthetic_path_ref(adapter_id: &str, native_session_id: &str, source_kind: &str) -> String {
    format!(
        "fixture://{}/{}.{}",
        sanitize_ref_token(adapter_id),
        sanitize_ref_token(native_session_id),
        match evidence_kind_from_source(source_kind) {
            "jsonl" => "jsonl",
            "json" => "json",
            "sqlite-row" => "sqlite",
            "markdown" => "md",
            "text" => "txt",
            _ => "bin",
        }
    )
}

fn normalize_token(value: &str) -> String {
    let mut normalized = String::new();
    let mut separator = false;
    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !normalized.is_empty() {
                normalized.push('-');
            }
            separator = false;
            normalized.push(character.to_ascii_lowercase());
        } else {
            separator = true;
        }
    }
    normalized
}

fn sanitize_ref_token(value: &str) -> String {
    let mut out = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
            out.push(character.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    if out.is_empty() {
        "unknown".to_string()
    } else {
        out.trim_matches('-').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_execution_events_and_evidence_independently() {
        assert_eq!(
            execution_event_kind("", "function_call_output"),
            "tool-result"
        );
        assert_eq!(execution_event_kind("shell-command", ""), "terminal");
        assert_eq!(layer_for_history_kind("diff"), SemanticLayer::Artifacts);
        assert_eq!(evidence_kind_from_source("DB"), "sqlite-row");
    }

    #[test]
    fn synthetic_reference_is_stable_and_path_free() {
        let path_ref = synthetic_path_ref("Claude Code", "session/one", "jsonl");
        assert_eq!(path_ref, "fixture://claude-code/session-one.jsonl");
        assert_eq!(hash_text("same"), hash_text("same"));
    }
}
