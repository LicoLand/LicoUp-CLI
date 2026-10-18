//! Canonical semantic conversation model for native agent history.
//!
//! Authority: `packages/contracts/client/semantic-conversation.schema.json`
//! Overview: `docs/architecture/README.md`

mod artifact_projection;
mod builder;
mod execution_projection;
mod io;
mod markdown;
mod model;
mod privacy;
mod thread_projection;
mod validation;

pub use crate::domain::conversation::event_semantics::{
    SemanticLayer, evidence_kind_from_source, execution_event_kind, hash_text,
    layer_for_history_kind, privacy_defaults, synthetic_path_ref,
};
pub use builder::{build_semantic_conversation, timeline_messages_from_semantic};
pub use execution_projection::execution_wire_message_from_tagged;
pub use io::{load_and_validate_fixture, materialize_semantic_documents};
pub use markdown::render_semantic_markdown;
pub use model::{
    SEMANTIC_JSON, SEMANTIC_KIND, SEMANTIC_MD, SEMANTIC_SCHEMA_VERSION, SemanticAuditInput,
    annotate_message_layer,
};
pub use thread_projection::thread_wire_message_from_tagged;
pub use validation::validate_semantic_conversation;

#[cfg(test)]
mod tests;
