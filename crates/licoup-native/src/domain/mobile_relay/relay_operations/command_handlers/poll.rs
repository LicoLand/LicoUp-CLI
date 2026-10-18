use super::super::mailbox::local_canonical_mailbox_tokens;
use super::super::station::{
    lease_transport_hint, station_context, station_lease_seconds, station_receive_limit,
};
use crate::domain::mobile_relay::secret_custody::{
    RuntimeSecretMaterial, load_config_with_runtime_secret_context,
};
use crate::domain::mobile_relay::support::CONFIG_SCHEMA_VERSION;
use crate::platform::badtower_station::BadTowerLeaseTransportHint;
use anyhow::{Result, anyhow};
use licoup_protocol_bindings::licoarc_relay::LicoArcRelayEnvelope;
use serde_json::{Value, json};

pub(in crate::domain::mobile_relay) struct StationPoll {
    pub(in crate::domain::mobile_relay) envelopes: Vec<LicoArcRelayEnvelope>,
    pub(in crate::domain::mobile_relay) lease_hint: BadTowerLeaseTransportHint,
}

pub fn commands_poll(params: &Value) -> Result<Value> {
    let (config, secret_context) = load_config_with_runtime_secret_context(params)?;
    commands_poll_with_config(params, &config, &secret_context.material)
}

pub(in crate::domain::mobile_relay) fn receive_station_envelopes_with_config(
    params: &Value,
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
) -> Result<StationPoll> {
    let mailbox_ids = local_canonical_mailbox_tokens(config, secret_material)?;
    let station = station_context(params, config)?;
    let current_mailbox_id = mailbox_ids
        .first()
        .ok_or_else(|| anyhow!("local canonical mailbox schedule is empty"))?;
    let lease_hint = station
        .transport
        .lease_mailbox(current_mailbox_id, station_lease_seconds(params))?;
    let mut envelopes = station
        .transport
        .receive_envelopes(current_mailbox_id, station_receive_limit(params)?)?;
    for previous_mailbox_id in mailbox_ids.iter().skip(1) {
        station
            .transport
            .lease_mailbox(previous_mailbox_id, station_lease_seconds(params))?;
        envelopes.extend(
            station
                .transport
                .receive_envelopes(previous_mailbox_id, station_receive_limit(params)?)?,
        );
    }
    Ok(StationPoll {
        envelopes,
        lease_hint,
    })
}

pub(in crate::domain::mobile_relay) fn commands_poll_with_config(
    params: &Value,
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
) -> Result<Value> {
    station_poll_projection(&receive_station_envelopes_with_config(
        params,
        config,
        secret_material,
    )?)
}

pub(in crate::domain::mobile_relay) fn station_poll_projection(
    poll: &StationPoll,
) -> Result<Value> {
    let envelopes = poll
        .envelopes
        .iter()
        .map(|envelope| {
            envelope
                .to_json()
                .and_then(|wire| serde_json::from_str::<Value>(&wire).map_err(Into::into))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({
        "ok": true,
        "schemaVersion": CONFIG_SCHEMA_VERSION,
        "envelopes": envelopes,
        "transportHint": lease_transport_hint(poll.lease_hint)
    }))
}
