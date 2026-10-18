use serde_json::json;

#[test]
fn snapshot_redaction_covers_json_text_metadata_and_reference_exceptions() {
    let authorization_canary = ["Bearer", "token-canary"].join(" ");
    let api_canary = ["api", "canary"].join("-");
    let query_canary = ["query", "canary"].join("-");
    let local_path = ["/", "private", "local", "path"].join("/");
    let refresh_canary = ["refresh", "canary"].join("-");
    let content = json!({
        "headers": {"Authorization": authorization_canary},
        "apiKey": api_canary,
        "secretRef": "secret://local/ref",
        "url": format!("https://example.invalid/?access_token={query_canary}"),
    })
    .to_string();
    let redacted = crate::redaction::redact_snapshot(
        &content,
        json!({
            "configPath": local_path,
            "refreshToken": refresh_canary
        }),
    )
    .unwrap();

    assert!(!redacted.content.contains("token-canary"));
    assert!(!redacted.content.contains("api-canary"));
    assert!(!redacted.content.contains("query-canary"));
    assert!(redacted.content.contains("secret://local/ref"));
    assert_eq!(
        redacted.metadata["configPath"],
        crate::policy::REDACTED_LOCAL_PATH
    );
    assert_eq!(
        redacted.metadata["refreshToken"],
        crate::policy::REDACTED_SECRET
    );
    assert_eq!(redacted.evidence["applied"], true);
}

#[test]
fn text_private_key_blocks_are_removed_in_full() {
    let private_key_block = [
        "-----BEGIN ",
        "PRIVATE KEY-----\n",
        "private-key-canary\n",
        "-----END ",
        "PRIVATE KEY-----",
    ]
    .concat();
    let content = format!("plain\n{private_key_block}\nplain");
    let redacted = crate::redaction::redact_snapshot(&content, json!({})).unwrap();

    assert!(!redacted.content.contains("private-key-canary"));
    assert!(
        redacted
            .content
            .contains(crate::policy::REDACTED_PRIVATE_KEY)
    );
}

#[test]
fn redaction_fails_closed_above_the_nesting_limit() {
    let mut payload = json!("leaf");
    for _ in 0..=crate::policy::MAX_REDACTION_DEPTH {
        payload = json!({"nested": payload});
    }

    assert!(crate::redaction::redact_activity_payload(payload).is_err());
}
