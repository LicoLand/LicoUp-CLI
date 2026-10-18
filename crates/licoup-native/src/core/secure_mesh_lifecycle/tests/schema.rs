use serde_json::json;

use super::super::schema::LifecycleServiceActionKind;

#[test]
fn lifecycle_schema_accepts_only_canonical_action_kinds() {
    let supported = [
        ("message_ttl_set", LifecycleServiceActionKind::MessageTtlSet),
        ("message_delete", LifecycleServiceActionKind::MessageDelete),
        (
            "screenshot_detected",
            LifecycleServiceActionKind::ScreenshotDetected,
        ),
        ("resend_request", LifecycleServiceActionKind::ResendRequest),
        ("typing_state", LifecycleServiceActionKind::TypingState),
        ("read_receipt", LifecycleServiceActionKind::ReadReceipt),
        ("ack_purge", LifecycleServiceActionKind::AckPurge),
    ];

    for (wire_name, expected) in supported {
        assert_eq!(
            LifecycleServiceActionKind::parse(&json!({"actionKind": wire_name})).unwrap(),
            expected
        );
        assert_eq!(expected.as_str(), wire_name);
    }

    let error = LifecycleServiceActionKind::parse(&json!({
        "actionKind": "arbitrary_remote_action"
    }))
    .unwrap_err();
    assert!(error.to_string().contains("unsupported"));
}
