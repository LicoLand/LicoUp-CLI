use super::test_support::*;

#[test]
fn secure_mesh_content_crypto_round_trips_supported_payload_kinds() {
    let key = key_fixture(7);
    for (index, payload) in [
        SecureMeshPlaintext::new(
            SecureMeshPayloadKind::Command,
            br#"{"op":"agent.message.send"}"#,
        ),
        SecureMeshPlaintext::new(SecureMeshPayloadKind::ResultPayload, br#"{"ok":true}"#),
        SecureMeshPlaintext::new(SecureMeshPayloadKind::Error, b"permission denied"),
        SecureMeshPlaintext::new(SecureMeshPayloadKind::FileChunk, b"file-bytes")
            .with_content_type("application/octet-stream"),
        SecureMeshPlaintext::new(
            SecureMeshPayloadKind::FileManifest,
            br#"{"name":"redacted.bin","chunks":1}"#,
        )
        .with_content_type("application/json"),
        SecureMeshPlaintext::new(
            SecureMeshPayloadKind::ServiceAction,
            br#"{"actionKind":"message_delete","messageHash":"sha256:redacted"}"#,
        )
        .with_content_type("application/json"),
        SecureMeshPlaintext::new(
            SecureMeshPayloadKind::TypingIndicator,
            br#"{"typingState":"started"}"#,
        )
        .with_content_type("application/json"),
        SecureMeshPlaintext::new(
            SecureMeshPayloadKind::ReadReceipt,
            br#"{"readUpToMessageDigest":"sha256:redacted"}"#,
        )
        .with_content_type("application/json"),
    ]
    .into_iter()
    .enumerate()
    {
        let nonce = [index as u8; CONTENT_NONCE_LEN];
        let sealed = seal_payload_with_nonce(&key, &context_fixture(), &payload, nonce).unwrap();
        let opened = open_payload(&key, &context_fixture(), &sealed, payload.kind).unwrap();
        assert_eq!(opened.kind, payload.kind);
        assert_eq!(opened.body, payload.body);
        assert_eq!(opened.content_type, payload.content_type);
        assert_eq!(opened.created_at, context_fixture().created_at);
        assert_eq!(opened.expires_at, context_fixture().expires_at);
    }
}
