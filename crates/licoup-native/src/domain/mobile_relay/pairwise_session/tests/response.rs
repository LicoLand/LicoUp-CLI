use super::super::response::{is_pairwise_replay_rejection, secure_result_response_summary};
use serde_json::json;

#[test]
fn response_summary_redacts_result_body() {
    let summary = secure_result_response_summary(&json!({
        "ok": true,
        "command": {
            "commandId": "command-test",
            "status": "completed",
            "resultEnvelope": {"ciphertext": "opaque"},
            "body": "must-not-project"
        },
        "ackPurge": {"purged": true}
    }));

    assert_eq!(summary["command"]["commandId"], "command-test");
    assert_eq!(summary["command"]["resultEnvelopePresent"], true);
    assert_eq!(summary["bodyRedacted"], true);
    assert!(summary["command"].get("body").is_none());
}

#[test]
fn replay_classifier_accepts_only_ratchet_replay_failures() {
    assert!(is_pairwise_replay_rejection("replay detected"));
    assert!(is_pairwise_replay_rejection("stale ratchet epoch"));
    assert!(is_pairwise_replay_rejection("stale chain index"));
    assert!(!is_pairwise_replay_rejection("transport timeout"));
}
