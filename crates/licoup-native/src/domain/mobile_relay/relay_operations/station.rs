use crate::domain::mobile_relay::config::{effective_station_base_url, validated_station_base_url};
use crate::domain::mobile_relay::support::text_param;
use crate::platform::badtower_station::{
    BadTowerDeletionTransportHint, BadTowerDeliveryTransportHint, BadTowerLeaseTransportHint,
    BadTowerStationTransport,
};
use anyhow::{Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(super) const DEFAULT_STATION_LEASE_SECONDS: u64 = 60;
pub(super) const DEFAULT_STATION_RECEIVE_LIMIT: u16 = 10;

pub(in crate::domain::mobile_relay) struct StationContext {
    pub(in crate::domain::mobile_relay) transport: BadTowerStationTransport,
}

pub(in crate::domain::mobile_relay) fn station_context(
    params: &Value,
    config: &Value,
) -> Result<StationContext> {
    ensure!(
        config
            .get("relayEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "mobile relay is disabled"
    );
    Ok(StationContext {
        transport: BadTowerStationTransport::new(station_base_url(params, config)?)?,
    })
}

pub(in crate::domain::mobile_relay) fn station_base_url(
    params: &Value,
    config: &Value,
) -> Result<String> {
    text_param(params, &["stationBaseUrl"])
        .map(|value| validated_station_base_url(&value))
        .transpose()?
        .map(Ok)
        .unwrap_or_else(|| effective_station_base_url(config))
}

pub(in crate::domain::mobile_relay) fn station_binding_digest(
    params: &Value,
    config: &Value,
) -> Result<String> {
    Ok(general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(station_base_url(params, config)?.as_bytes())))
}

pub(in crate::domain::mobile_relay) fn station_lease_seconds(params: &Value) -> u64 {
    params
        .get("leaseSeconds")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_STATION_LEASE_SECONDS)
}

pub(super) fn station_receive_limit(params: &Value) -> Result<u16> {
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(DEFAULT_STATION_RECEIVE_LIMIT));
    u16::try_from(limit).map_err(|_| anyhow!("mobile relay station receive limit is invalid"))
}

pub(in crate::domain::mobile_relay) fn lease_transport_hint(
    hint: BadTowerLeaseTransportHint,
) -> Value {
    json!({
        "stationReportedLeased": hint.station_reported_leased()
    })
}

pub(in crate::domain::mobile_relay) fn delivery_transport_hint(
    hint: BadTowerDeliveryTransportHint,
) -> Value {
    json!({
        "stationReportedAccepted": hint.station_reported_accepted(),
        "stationReportedDuplicate": hint.station_reported_duplicate()
    })
}

pub(in crate::domain::mobile_relay) fn deletion_transport_hint(
    hint: BadTowerDeletionTransportHint,
) -> Value {
    json!({
        "stationReportedAcknowledged": hint.station_reported_acknowledged()
    })
}
