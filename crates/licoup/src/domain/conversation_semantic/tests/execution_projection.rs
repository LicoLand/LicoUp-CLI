use serde_json::json;

use super::super::execution_projection::{
    append_timeline_messages, execution_wire_message_from_tagged,
};

#[test]
fn execution_wire_projection_preserves_bounded_card_and_usage_metadata() {
    let wire = execution_wire_message_from_tagged(&json!({
        "id": "execution-1",
        "role": "reasoning",
        "cardType": "reasoning",
        "cardTitle": "Reasoning",
        "text": "Provider-authored summary",
        "providerSummary": true,
        "createdAt": "2026-01-01T00:00:00Z",
        "usage": {"outputTokens": 3}
    }))
    .expect("execution wire message");
    assert_eq!(wire["layer"], "execution");
    assert_eq!(wire["providerSummary"], true);
    assert_eq!(wire["cardSubtitle"], "Reasoning summary");
    assert_eq!(wire["usage"]["outputTokens"], 3);
}

#[test]
fn execution_timeline_projection_keeps_metadata_collapsed() {
    let semantic = json!({
        "execution": [{
            "id": "metadata-1",
            "eventKind": "event",
            "sourceItemType": "metadata",
            "title": "Metadata",
            "summary": "Sensitive details hidden",
            "createdAt": "",
            "collapsed": true
        }]
    });
    let mut messages = Vec::new();
    append_timeline_messages(&semantic, &mut messages);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "metadata");
    assert_eq!(messages[0]["cardType"], "metadata");
    assert_eq!(messages[0]["collapsed"], true);
}
