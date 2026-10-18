use anyhow::{Result, ensure};
use serde_json::{Value, json};

use super::policy::LifecyclePolicyDecision;
use super::schema::{SECURE_MESH_LIFECYCLE_CONTENT_TYPE, SECURE_MESH_LIFECYCLE_STATUS};
use crate::core::secure_mesh_crypto::{
    OpenedSecureMeshPayload, SecureMeshPayloadKind, SecureMeshPlaintext,
};

pub(super) fn project_policy_decision(decision: LifecyclePolicyDecision) -> Result<Value> {
    let projected = json!({
        "ok": true,
        "protocolVersion": crate::core::secure_mesh::SECURE_MESH_PROTOCOL_VERSION,
        "lifecycleStatus": SECURE_MESH_LIFECYCLE_STATUS,
        "actionKind": decision.action_kind,
        "scope": decision.scope,
        "servicePolicy": decision.service_policy,
        "requiresPairwiseOrMlsEnvelope": true,
        "serverVisiblePlaintextAllowed": false,
        "metadataRedacted": true,
        "bodyRedacted": true,
        "keyMaterial": "redacted"
    });
    require_protected_projection(&projected)?;
    Ok(projected)
}

pub(super) fn protected_plaintext(projected: &Value) -> Result<SecureMeshPlaintext> {
    require_protected_projection(projected)?;
    Ok(SecureMeshPlaintext::new(
        SecureMeshPayloadKind::ServiceAction,
        serde_json::to_vec(projected)?,
    )
    .with_content_type(SECURE_MESH_LIFECYCLE_CONTENT_TYPE))
}

pub(super) fn decode_protected_projection(
    opened: &OpenedSecureMeshPayload,
    transport: &str,
) -> Result<Value> {
    ensure!(
        opened.content_type.as_deref() == Some(SECURE_MESH_LIFECYCLE_CONTENT_TYPE),
        "secure mesh lifecycle {transport} content type mismatch"
    );
    let value: Value = serde_json::from_slice(&opened.body)?;
    require_protected_projection(&value)?;
    Ok(value)
}

fn require_protected_projection(projected: &Value) -> Result<()> {
    ensure!(
        projected
            .get("requiresPairwiseOrMlsEnvelope")
            .and_then(Value::as_bool)
            == Some(true),
        "secure mesh lifecycle action must require a protected envelope"
    );
    ensure!(
        projected
            .get("serverVisiblePlaintextAllowed")
            .and_then(Value::as_bool)
            == Some(false),
        "secure mesh lifecycle projection must forbid server-visible plaintext"
    );
    Ok(())
}
