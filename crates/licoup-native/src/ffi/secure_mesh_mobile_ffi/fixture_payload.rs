use anyhow::ensure;
use serde_json::{Value, json};

pub(super) fn native_payload_crypto_fixture() -> anyhow::Result<Value> {
    let key = crate::core::secure_mesh_crypto::ContentKey::from_bytes([31u8; 32]);
    let context = crate::core::secure_mesh_crypto::SecureMeshContentContext::new(
        "env_mobile_native_payload_fixture",
        "msg_mobile_native_payload_fixture",
        "mailbox_mobile_native_payload_fixture",
        "desktop-native-payload-fixture",
        "mobile-native-payload-fixture",
        "session_mobile_native_payload_fixture",
        "2026-01-01T00:00:00.000Z",
        "2026-01-01T00:10:00.000Z",
    );
    let body = br#"{"op":"native-payload-crypto-fixture"}"#;
    let plaintext = crate::core::secure_mesh_crypto::SecureMeshPlaintext::new(
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
        body,
    )
    .with_content_type("application/json");
    let sealed = crate::core::secure_mesh_crypto::seal_payload(&key, &context, &plaintext)?;
    let opened = crate::core::secure_mesh_crypto::open_payload(
        &key,
        &context,
        &sealed,
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
    )?;
    ensure!(
        opened.body == body,
        "native payload crypto self-test failed"
    );
    Ok(json!({"ok": true, "bodyRedacted": true}))
}
