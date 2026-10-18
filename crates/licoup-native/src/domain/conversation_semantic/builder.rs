use anyhow::Result;
use serde_json::{Value, json};

use crate::domain::conversation::event_semantics::{
    SemanticLayer, evidence_kind_from_source, hash_text, privacy_defaults, synthetic_path_ref,
};

use super::artifact_projection::artifact_from_message;
use super::execution_projection::{
    append_timeline_messages as append_execution_timeline, execution_event_from_message,
};
use super::model::{SEMANTIC_KIND, SEMANTIC_SCHEMA_VERSION, SemanticAuditInput};
use super::privacy::{redact_path_ref, sanitize_default_view_text};
use super::thread_projection::{
    append_timeline_messages as append_thread_timeline, thread_event_from_message,
};
use super::validation::validate_semantic_conversation;

pub fn build_semantic_conversation(
    messages: &[Value],
    audit: SemanticAuditInput<'_>,
) -> Result<Value> {
    let mut thread = Vec::new();
    let mut execution = Vec::new();
    let mut artifacts = Vec::new();
    let mut parse_warnings = audit.parse_warnings.to_vec();

    for message in messages {
        let layer = message
            .get("layer")
            .and_then(Value::as_str)
            .unwrap_or_else(|| infer_layer_from_message(message));
        match layer {
            "thread" => {
                if let Some(mut event) = thread_event_from_message(message) {
                    if let Some(object) = event.as_object_mut()
                        && let Some(text) = object.get("text").and_then(Value::as_str)
                    {
                        object.insert("text".to_string(), json!(sanitize_default_view_text(text)));
                    }
                    if event
                        .get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.trim().is_empty())
                    {
                        thread.push(event);
                    } else {
                        parse_warnings
                            .push("thread candidate dropped after privacy filter".to_string());
                    }
                } else {
                    parse_warnings
                        .push("thread candidate dropped after privacy filter".to_string());
                }
            }
            "execution" => {
                if let Some(mut event) = execution_event_from_message(message) {
                    if let Some(object) = event.as_object_mut()
                        && let Some(text) = object.get("summary").and_then(Value::as_str)
                    {
                        object.insert(
                            "summary".to_string(),
                            json!(sanitize_default_view_text(text)),
                        );
                    }
                    execution.push(event);
                }
            }
            "artifacts" => {
                let mut event = artifact_from_message(message);
                if let Some(object) = event.as_object_mut()
                    && let Some(reference) = object.get("ref").and_then(Value::as_str)
                {
                    object.insert("ref".to_string(), json!(redact_path_ref(reference)));
                }
                artifacts.push(event);
            }
            "audit" | "raw" => parse_warnings.push(format!(
                "{}-layer message excluded from default timeline projection",
                layer
            )),
            other => parse_warnings.push(format!("unknown layer `{other}` ignored")),
        }
    }

    let path_ref = if audit.path_ref.trim().is_empty() {
        synthetic_path_ref(audit.adapter_id, audit.native_session_id, audit.source_kind)
    } else {
        redact_path_ref(audit.path_ref)
    };
    let content_hash = if audit.content_hash.trim().is_empty() {
        hash_text(&format!(
            "{}|{}|{}",
            audit.adapter_id, audit.native_session_id, path_ref
        ))
    } else {
        audit.content_hash.to_string()
    };
    let evidence = json!({
        "kind": evidence_kind_from_source(audit.source_kind),
        "pathRef": path_ref,
        "contentHash": content_hash,
        "byteLength": audit.byte_length
    });

    let semantic = json!({
        "schemaVersion": SEMANTIC_SCHEMA_VERSION,
        "kind": SEMANTIC_KIND,
        "readOnly": true,
        "privacyDefaults": privacy_defaults(),
        "thread": thread,
        "execution": execution,
        "artifacts": artifacts,
        "audit": {
            "adapterId": audit.adapter_id,
            "adapterLabel": audit.adapter_label,
            "hostApp": audit.host_app,
            "hostAppLabel": audit.host_app_label,
            "sourceClient": audit.source_client,
            "sourceKind": audit.source_kind,
            "nativeSessionId": audit.native_session_id,
            "importMode": "precise-adapter",
            "sourceEvidence": evidence.clone(),
            "parseWarnings": parse_warnings,
            "redactionStatus": audit.redaction_status,
            "validationStatus": audit.validation_status,
            "createdAt": audit.created_at,
            "updatedAt": audit.updated_at
        },
        "raw": {
            "evidenceRefs": [evidence]
        }
    });
    validate_semantic_conversation(&semantic)?;
    Ok(semantic)
}

pub fn timeline_messages_from_semantic(semantic: &Value) -> Vec<Value> {
    let mut messages = Vec::new();
    append_thread_timeline(semantic, &mut messages);
    append_execution_timeline(semantic, &mut messages);
    messages
}

fn infer_layer_from_message(message: &Value) -> &'static str {
    if message
        .get("cardType")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
    {
        return SemanticLayer::Execution.as_str();
    }
    let role = message.get("role").and_then(Value::as_str).unwrap_or("");
    match role {
        "user" | "human" | "assistant" | "agent" | "model" | "ai" => SemanticLayer::Thread.as_str(),
        _ => SemanticLayer::Execution.as_str(),
    }
}
