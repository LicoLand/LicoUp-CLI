use super::super::payload::secure_command_payload;
use crate::domain::mobile_relay::secret_custody::RuntimeSecretMaterial;
use serde_json::json;

#[test]
fn command_payload_requires_bound_local_and_peer_identity() {
    let error = secure_command_payload(
        &json!({}),
        &RuntimeSecretMaterial::new(),
        "agent.sessions.list",
        None,
        "default",
        json!({}),
    )
    .err()
    .expect("missing endpoint state must be rejected");
    assert!(error.to_string().contains("endpoint state is missing"));
}
