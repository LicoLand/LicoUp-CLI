use serde_json::{Value, json};

use crate::domain::conversation::event_semantics::SemanticLayer;

pub const SEMANTIC_SCHEMA_VERSION: u64 = 1;
pub const SEMANTIC_KIND: &str = "semantic-conversation";
pub const SEMANTIC_JSON: &str = "semantic.json";
pub const SEMANTIC_MD: &str = "semantic.md";

pub struct SemanticAuditInput<'a> {
    pub adapter_id: &'a str,
    pub adapter_label: &'a str,
    pub host_app: &'a str,
    pub host_app_label: &'a str,
    pub source_client: &'a str,
    pub source_kind: &'a str,
    pub native_session_id: &'a str,
    pub path_ref: &'a str,
    pub content_hash: &'a str,
    pub byte_length: u64,
    pub parse_warnings: &'a [String],
    pub redaction_status: &'a str,
    pub validation_status: &'a str,
    pub created_at: &'a str,
    pub updated_at: &'a str,
}

pub fn annotate_message_layer(message: &mut Value, layer: SemanticLayer) {
    if let Some(object) = message.as_object_mut() {
        object.insert("layer".to_string(), json!(layer.as_str()));
    }
}
