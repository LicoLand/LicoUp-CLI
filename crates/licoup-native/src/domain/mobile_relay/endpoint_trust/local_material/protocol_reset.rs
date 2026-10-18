use crate::domain::mobile_relay::support::MOBILE_RELAY_E2EE_PROTOCOL_VERSION;
use anyhow::{Result, ensure};
use serde_json::Value;

pub(in crate::domain::mobile_relay) fn ensure_local_pairwise_protocol_compatible(
    config: &Value,
) -> Result<()> {
    let incompatible = config
        .get("mobileRelayE2ee")
        .and_then(|state| state.get("protocolVersion"))
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|protocol| protocol != MOBILE_RELAY_E2EE_PROTOCOL_VERSION);
    ensure!(
        !incompatible,
        "mobile relay pairwise protocol requires explicit startup migration"
    );
    Ok(())
}
