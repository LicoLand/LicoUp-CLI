use super::envelope::relay_envelope_from_value;
use crate::domain::mobile_relay::support::SECURE_MESH_ENVELOPE_COMMAND;
use anyhow::Result;
use licoup_protocol_bindings::licoarc_relay::LicoArcRelayEnvelope;
use serde_json::{Value, json};

pub(in crate::domain::mobile_relay) fn relay_envelope_from_delivery(
    value: &Value,
) -> Result<LicoArcRelayEnvelope> {
    relay_envelope_from_value(value)
}

pub(in crate::domain::mobile_relay) fn local_command_from_relay_delivery(
    value: &Value,
) -> Result<Value> {
    let envelope = relay_envelope_from_delivery(value)?;
    Ok(json!({
        "commandId": envelope.envelope_id(),
        "type": SECURE_MESH_ENVELOPE_COMMAND,
        "envelope": serde_json::from_str::<Value>(&envelope.to_json()?)?
    }))
}
