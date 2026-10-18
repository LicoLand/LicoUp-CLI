use super::super::mailbox::local_canonical_mailbox_tokens;
use super::super::station::{lease_transport_hint, station_context, station_lease_seconds};
use crate::domain::mobile_relay::secret_custody::{
    RuntimeSecretContext, load_config_with_runtime_secret_context,
};
use crate::domain::mobile_relay::support::CONFIG_SCHEMA_VERSION;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};

pub fn pc_check_in(params: &Value) -> Result<Value> {
    let (config, secret_context) = load_config_with_runtime_secret_context(params)?;
    pc_check_in_with_context(params, &config, &secret_context)
}

pub(in crate::domain::mobile_relay) fn pc_check_in_with_context(
    params: &Value,
    config: &Value,
    secret_context: &RuntimeSecretContext,
) -> Result<Value> {
    let mailbox_ids = local_canonical_mailbox_tokens(config, &secret_context.material)?;
    let station = station_context(params, config)?;
    let current_mailbox_id = mailbox_ids
        .first()
        .ok_or_else(|| anyhow!("local canonical mailbox schedule is empty"))?;
    let hint = station
        .transport
        .lease_mailbox(current_mailbox_id, station_lease_seconds(params))?;
    for previous_mailbox_id in mailbox_ids.iter().skip(1) {
        station
            .transport
            .lease_mailbox(previous_mailbox_id, station_lease_seconds(params))?;
    }
    Ok(json!({
        "ok": true,
        "schemaVersion": CONFIG_SCHEMA_VERSION,
        "transportHint": lease_transport_hint(hint)
    }))
}
