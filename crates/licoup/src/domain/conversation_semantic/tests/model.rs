use serde_json::json;

use super::super::model::{
    SEMANTIC_JSON, SEMANTIC_KIND, SEMANTIC_MD, SEMANTIC_SCHEMA_VERSION, annotate_message_layer,
};
use crate::domain::conversation::event_semantics::SemanticLayer;

#[test]
fn model_constants_and_layer_annotation_are_stable() {
    assert_eq!(SEMANTIC_SCHEMA_VERSION, 1);
    assert_eq!(SEMANTIC_KIND, "semantic-conversation");
    assert_eq!(SEMANTIC_JSON, "semantic.json");
    assert_eq!(SEMANTIC_MD, "semantic.md");

    let mut message = json!({"id": "message-1"});
    annotate_message_layer(&mut message, SemanticLayer::Execution);
    assert_eq!(message["layer"], "execution");
}
