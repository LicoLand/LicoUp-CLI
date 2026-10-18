//! Session construction, title selection, source identity, and evidence metadata.

use super::*;

pub(super) fn session_from_messages(
    adapter: HistoryAdapter,
    path: &Path,
    metadata: &fs::Metadata,
    source_kind: &str,
    native_session_id: String,
    messages: Vec<Value>,
) -> Value {
    session_from_messages_with_title(
        adapter,
        path,
        metadata,
        source_kind,
        native_session_id,
        messages,
        None,
    )
}

pub(super) fn session_from_messages_with_title(
    adapter: HistoryAdapter,
    path: &Path,
    metadata: &fs::Metadata,
    source_kind: &str,
    native_session_id: String,
    messages: Vec<Value>,
    explicit_title: Option<String>,
) -> Value {
    let updated_at = system_time(metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH));
    let created_at = system_time(metadata.created().unwrap_or(SystemTime::UNIX_EPOCH));
    let mut tagged_messages = messages;
    for message in &mut tagged_messages {
        super::message_projection::normalize_generated_metadata_message(message);
    }
    let source_client = source_client_for_session(adapter, path, source_kind, &tagged_messages);
    let host_app = host_app_for_path(adapter, path);
    let host_app_label_value = host_app_label(&host_app);
    let source_client_label_value = source_client_label(&source_client);
    let source_label = source_label(&host_app, &source_client);
    let title = explicit_title
        .as_deref()
        .and_then(|title| normalized_explicit_title(adapter, title))
        .or_else(|| title_from_messages(&tagged_messages))
        .unwrap_or_else(|| fallback_conversation_title(adapter, path));
    for message in &mut tagged_messages {
        ensure_message_semantic_layer(message);
        // A source without any message timestamp keeps one stable session
        // timestamp instead of an empty key. JSONL transcripts are backfilled
        // with interpolated transcript times afterwards by the parser.
        if message
            .get("createdAt")
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().is_empty())
        {
            message["createdAt"] = json!(updated_at.clone());
        }
    }
    let path_display = display_path(path);
    let source_bytes = metadata.len();
    let semantic = match crate::domain::conversation_semantic::build_semantic_conversation(
        &tagged_messages,
        crate::domain::conversation_semantic::SemanticAuditInput {
            adapter_id: adapter.id(),
            adapter_label: adapter.label(),
            host_app: &host_app,
            host_app_label: &host_app_label_value,
            source_client: &source_client,
            source_kind,
            native_session_id: &native_session_id,
            path_ref: &path_display,
            content_hash: "",
            byte_length: source_bytes,
            parse_warnings: &[],
            redaction_status: "applied",
            validation_status: "unchecked",
            created_at: &created_at,
            updated_at: &updated_at,
        },
    ) {
        Ok(value) => value,
        Err(error) => {
            let path_ref = crate::domain::conversation_semantic::synthetic_path_ref(
                adapter.id(),
                &native_session_id,
                source_kind,
            );
            let content_hash = crate::domain::conversation_semantic::hash_text(&format!(
                "semantic-fallback|{}|{}",
                adapter.id(),
                native_session_id
            ));
            let evidence_kind =
                crate::domain::conversation_semantic::evidence_kind_from_source(source_kind);
            json!({
                "schemaVersion": crate::domain::conversation_semantic::SEMANTIC_SCHEMA_VERSION,
                "kind": crate::domain::conversation_semantic::SEMANTIC_KIND,
                "readOnly": true,
                "privacyDefaults": crate::domain::conversation_semantic::privacy_defaults(),
                "thread": [],
                "execution": [],
                "artifacts": [],
                "audit": {
                    "adapterId": adapter.id(),
                    "adapterLabel": adapter.label(),
                    "hostApp": host_app.clone(),
                    "hostAppLabel": host_app_label_value,
                    "sourceClient": source_client.clone(),
                    "sourceKind": source_kind,
                    "nativeSessionId": native_session_id.clone(),
                    "importMode": "precise-adapter",
                    "sourceEvidence": {
                        "kind": evidence_kind,
                        "pathRef": path_ref.clone(),
                        "contentHash": content_hash.clone(),
                        "byteLength": source_bytes
                    },
                    "parseWarnings": [format!("semantic assembly fallback: {error}")],
                    "redactionStatus": "applied",
                    "validationStatus": "failed",
                    "createdAt": created_at.clone(),
                    "updatedAt": updated_at.clone()
                },
                "raw": {
                    "evidenceRefs": [{
                        "kind": evidence_kind,
                        "pathRef": path_ref,
                        "contentHash": content_hash,
                        "byteLength": source_bytes
                    }]
                }
            })
        }
    };
    let mut projected = Vec::new();
    for message in &tagged_messages {
        let layer = message
            .get("layer")
            .and_then(Value::as_str)
            .unwrap_or("execution");
        match layer {
            "thread" => {
                if let Some(event) =
                    crate::domain::conversation_semantic::thread_wire_message_from_tagged(message)
                {
                    projected.push(event);
                }
            }
            "execution" => {
                let role = message.get("role").and_then(Value::as_str).unwrap_or("");
                let card_type = message
                    .get("cardType")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if role == "subagent" || card_type == "subagent" {
                    let mut card = message.clone();
                    crate::domain::conversation_semantic::annotate_message_layer(
                        &mut card,
                        crate::domain::conversation_semantic::SemanticLayer::Execution,
                    );
                    projected.push(card);
                } else if let Some(event) =
                    crate::domain::conversation_semantic::execution_wire_message_from_tagged(
                        message,
                    )
                {
                    projected.push(event);
                }
            }
            _ => {}
        }
    }
    if projected.is_empty() {
        projected =
            crate::domain::conversation_semantic::timeline_messages_from_semantic(&semantic);
    }
    json!({
        "id": session_id(adapter.id(), path, &native_session_id),
        "agentId": adapter.id(),
        "adapterId": adapter.id(),
        "adapterLabel": adapter.label(),
        "sourceTool": source_client.clone(),
        "sourceClient": source_client,
        "sourceClientLabel": source_client_label_value,
        "hostApp": host_app,
        "hostAppLabel": host_app_label_value,
        "sourceLabel": source_label,
        "sourceKind": source_kind,
        "sourcePath": path_display,
        "nativeSessionId": native_session_id,
        "importMode": "precise-adapter",
        "title": title,
        "createdAt": created_at,
        "updatedAt": updated_at,
        "sourceRevision": source_revision(path, metadata),
        "native": true,
        "readOnly": true,
        "messageCount": projected.len(),
        "semantic": semantic,
        "messages": projected
    })
}

/// Reuse the source stat facts that invalidate the browse cache. Display
/// timestamps can be rounded or provider-owned and are not content revisions.
fn source_revision(path: &Path, metadata: &fs::Metadata) -> String {
    let mut revision = format!(
        "{}:{}",
        metadata.len(),
        super::projection_cache::modified_ns(metadata)
    );
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "db" | "sqlite" | "sqlite3" | "vscdb"))
    {
        let mut wal = path.as_os_str().to_os_string();
        wal.push("-wal");
        if let Ok(metadata) = fs::metadata(Path::new(&wal)) {
            revision.push_str(&format!(
                ";{}:{}",
                metadata.len(),
                super::projection_cache::modified_ns(&metadata)
            ));
        }
    }
    revision
}

pub(super) fn ensure_message_semantic_layer(message: &mut Value) {
    if message
        .get("layer")
        .and_then(Value::as_str)
        .is_some_and(|layer| !layer.trim().is_empty())
    {
        return;
    }
    let role = message.get("role").and_then(Value::as_str).unwrap_or("");
    let card_type = message
        .get("cardType")
        .and_then(Value::as_str)
        .unwrap_or("");
    let layer = if matches!(role, "transcript" | "record") {
        crate::domain::conversation_semantic::SemanticLayer::Thread
    } else if !card_type.is_empty()
        || matches!(
            role,
            "tool_call" | "tool_result" | "reasoning" | "metadata" | "error" | "event" | "subagent"
        )
    {
        crate::domain::conversation_semantic::SemanticLayer::Execution
    } else if matches!(
        role,
        "user"
            | "human"
            | "assistant"
            | "agent"
            | "model"
            | "ai"
            | "planner-response"
            | "planner_response"
            | "generic"
    ) {
        crate::domain::conversation_semantic::SemanticLayer::Thread
    } else {
        crate::domain::conversation_semantic::SemanticLayer::Execution
    };
    crate::domain::conversation_semantic::annotate_message_layer(message, layer);
}

pub(super) fn title_from_messages(messages: &[Value]) -> Option<String> {
    for preferred_role in ["user", "human"] {
        if let Some(title) = messages.iter().find_map(|message| {
            let role = message.get("role").and_then(Value::as_str).unwrap_or("");
            if role == preferred_role {
                message
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|text| title_candidate_text(text))
                    .map(title_from_message_text)
            } else {
                None
            }
        }) {
            return Some(title);
        }
    }
    messages.iter().find_map(|message| {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        if !matches!(role, "transcript" | "record") {
            return None;
        }
        let text = message.get("text").and_then(Value::as_str)?;
        title_from_conversation_marker(text)
    })
}

pub(super) fn title_from_message_text(text: &str) -> String {
    let cleaned = strip_generated_context_blocks(text);
    let source = if cleaned.trim().is_empty() {
        text
    } else {
        cleaned.as_str()
    };
    if let Some(title) = title_from_conversation_marker(source) {
        return title;
    }
    for line in source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let lower = line.to_ascii_lowercase();
        for prefix in ["user:", "human:", "prompt:", "question:"] {
            if lower.starts_with(prefix) {
                return title_from_text(line[prefix.len()..].trim());
            }
        }
    }
    title_from_text(source)
}

pub(super) fn title_from_conversation_marker(text: &str) -> Option<String> {
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let field_value = line.split_once(':').map(|(_, value)| value.trim());
        let candidates = [Some(line), field_value];
        for candidate in candidates.into_iter().flatten() {
            if let Some(title) = title_from_conversation_marker_line(candidate) {
                return Some(title);
            }
        }
    }
    None
}

pub(super) fn title_from_conversation_marker_line(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    for prefix in [
        "user:",
        "human:",
        "prompt:",
        "question:",
        "message:",
        "user message:",
        "human message:",
        "prompt message:",
        "question message:",
    ] {
        if lower.starts_with(prefix) {
            let title = line[prefix.len()..].trim();
            if title_candidate_text(title) {
                return Some(title_from_text(title));
            }
        }
    }
    None
}

pub(super) fn title_candidate_text(text: &str) -> bool {
    let cleaned = strip_generated_context_blocks(text);
    let trimmed = if cleaned.trim().is_empty() {
        text.trim()
    } else {
        cleaned.trim()
    };
    !trimmed.is_empty()
        && !metadata_like_text(trimmed)
        && !generated_control_text(trimmed)
        && !background_context_prompt_text(trimmed)
}

pub(super) fn meaningful_explicit_title(title: &str) -> bool {
    let trimmed = title.trim();
    title_candidate_text(trimmed)
        && !looks_like_generated_identity(trimmed)
        && !looks_like_generated_status_title(trimmed)
}

pub(super) fn normalized_explicit_title(adapter: HistoryAdapter, title: &str) -> Option<String> {
    let cleaned = if matches!(adapter, HistoryAdapter::Antigravity) {
        strip_antigravity_artifact_noise(&extract_antigravity_user_request(title))
    } else {
        title.trim().to_string()
    };
    if meaningful_explicit_title(&cleaned) {
        Some(title_from_text(&cleaned))
    } else {
        None
    }
}

pub(super) fn extract_conversation_title(value: &Value) -> Option<String> {
    find_string(
        value,
        &[
            "thread_name",
            "threadName",
            "title",
            "name",
            "conversationTitle",
            "chatTitle",
            "sessionTitle",
            "summary",
        ],
    )
    .filter(|title| meaningful_explicit_title(title))
}

pub(super) fn fallback_conversation_title(adapter: HistoryAdapter, path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if !stem.is_empty() && !looks_like_generated_identity(stem) {
        return title_from_text(stem);
    }
    format!("{} conversation", adapter.label())
}

pub(super) fn metadata_like_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    if generated_control_text(trimmed) {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("cwd:")
        || lower.starts_with("workingdirectory:")
        || lower.starts_with("projectpath:")
        || lower.starts_with("codex event:")
        || lower.starts_with("<environment_context>")
        || lower.starts_with("<apps_instructions>")
        || lower.starts_with("<apps-instructions>")
    {
        return true;
    }
    let line_count = trimmed.lines().count().max(1);
    let key_value_lines = trimmed
        .lines()
        .filter(|line| {
            let line = line.trim();
            line.contains(':') && !line.contains(' ') && line.len() < 80
        })
        .count();
    key_value_lines == line_count && line_count <= 4
}

pub(super) fn looks_like_generated_status_title(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    lower.starts_with("updated ")
        || lower.starts_with("created ")
        || lower.starts_with("deleted ")
        || lower.starts_with("renamed ")
        || lower.starts_with("moved ")
        || lower.starts_with("indexed ")
        || lower.starts_with("the conversation has been cleared")
        || lower.starts_with("conversation has been cleared")
}

pub(super) fn looks_like_generated_identity(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return true;
    }
    if looks_like_uuid(value) {
        return true;
    }
    let compact = value.replace(['-', '_'], "");
    compact.len() >= 16 && compact.chars().all(|ch| ch.is_ascii_hexdigit())
}

pub(super) fn source_client_for_session(
    adapter: HistoryAdapter,
    path: &Path,
    source_kind: &str,
    messages: &[Value],
) -> String {
    let evidence = source_evidence_text(path, messages);
    if evidence.contains("github.copilot")
        || evidence.contains("copilot-chat")
        || evidence.contains("chat-session-resources")
    {
        return "copilot".to_string();
    }
    if evidence.contains("kilo-code")
        || evidence.contains("kilocode")
        || evidence.contains("/kilo/")
    {
        return "kilo-code".to_string();
    }
    if adapter == HistoryAdapter::Cursor
        && matches!(source_kind, "cursor-cli-chats" | "cursor-cli-projects")
    {
        return "cursor-agent".to_string();
    }
    adapter.id().to_string()
}

pub(super) fn source_evidence_text(path: &Path, messages: &[Value]) -> String {
    let mut parts = vec![path.to_string_lossy().replace('\\', "/")];
    for message in messages.iter().take(8) {
        for key in ["sourcePath", "sourceKey", "sourceTable"] {
            if let Some(text) = message.get(key).and_then(Value::as_str) {
                parts.push(text.replace('\\', "/"));
            }
        }
    }
    parts.join("\n").to_ascii_lowercase()
}

pub(super) fn host_app_for_path(adapter: HistoryAdapter, path: &Path) -> String {
    let path_text = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if path_text.contains("/library/application support/code/")
        || path_text.contains("/.config/code/")
        || path_text.contains("/appdata/roaming/code/")
    {
        return "vscode".to_string();
    }
    if path_text.contains("/library/application support/cursor/")
        || path_text.contains("/.config/cursor/")
    {
        return "cursor".to_string();
    }
    if path_text.contains("antigravity ide") || path_text.contains("/.gemini/antigravity") {
        return "antigravity".to_string();
    }
    adapter.id().to_string()
}

pub(super) fn source_label(host_app: &str, source_client: &str) -> String {
    let source = source_client_display(source_client);
    if !host_app.is_empty() && host_app != source_client {
        format!("{}: {}", host_app_display(host_app), source)
    } else {
        source.to_string()
    }
}

pub(super) fn source_client_label(source_client: &str) -> &'static str {
    match source_client {
        "antigravity" => "Antigravity",
        "claude-code" => "Claude Code",
        "code" => "VS Code",
        "codex" => "Codex CLI",
        "copilot" => "GitHub Copilot",
        "cursor" => "Cursor",
        "cursor-agent" => "Cursor Agent CLI",
        "hermes" => "Hermes Agent",
        "kilo-code" => "Kilo Code",
        "kimi-code" => "Kimi Code",
        "openclaw" => "OpenClaw",
        "opencode" => "OpenCode",
        "pi" => "Pi Agent",
        _ => "Native Conversation",
    }
}

pub(super) fn host_app_label(host_app: &str) -> &'static str {
    match host_app {
        "antigravity" => "Antigravity",
        "claude-code" => "Claude Code",
        "code" | "vscode" => "VS Code",
        "codex" => "Codex",
        "copilot" => "GitHub Copilot",
        "cursor" => "Cursor",
        "hermes" => "Hermes Agent",
        "kilo-code" => "Kilo Code",
        "kimi-code" => "Kimi Code",
        "openclaw" => "OpenClaw",
        "opencode" => "OpenCode",
        "pi" => "Pi Agent",
        _ => "Native Host",
    }
}

pub(super) fn source_client_display(source_client: &str) -> &'static str {
    match source_client {
        "claude-code" => "claude code",
        "code" | "vscode" => "vscode",
        "copilot" => "copilot",
        "kilo-code" => "kilo code",
        "openclaw" => "openclaw",
        "opencode" => "opencode",
        "antigravity" => "antigravity",
        "codex" => "codex cli",
        "cursor" => "cursor",
        "cursor-agent" => "cursor agent cli",
        "hermes" => "hermes",
        "kimi-code" => "kimi code",
        "pi" => "pi",
        _ => "conversation",
    }
}

pub(super) fn host_app_display(host_app: &str) -> &'static str {
    match host_app {
        "code" | "vscode" => "vscode",
        "kilo-code" => "kilo code",
        "claude-code" => "claude code",
        "openclaw" => "openclaw",
        "opencode" => "opencode",
        "antigravity" => "antigravity",
        "codex" => "codex",
        "copilot" => "copilot",
        "cursor" => "cursor",
        "hermes" => "hermes",
        "kimi-code" => "kimi code",
        "pi" => "pi",
        _ => "native",
    }
}
