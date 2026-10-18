use super::super::protocol_reset::ensure_local_pairwise_protocol_compatible;
use serde_json::json;

#[test]
fn incompatible_pairwise_protocol_fails_without_mutating_bound_local_state() {
    let config = json!({
        "mobileRelayE2ee": {"protocolVersion": "retired-protocol"},
        "pairingId": "pairing",
        "pcToken": "pc",
        "mobileToken": "mobile",
        "paired": true,
        "relayEnabled": true,
        "lastPairingCode": "code",
        "lastPairingExpiresAt": "later",
        "mobileRelayPairingInvite": {"present": true},
        "pairedDevices": [],
        "secretStorageStatus": {"present": true}
    });

    let preserved = config.clone();
    assert!(ensure_local_pairwise_protocol_compatible(&config).is_err());
    assert_eq!(config, preserved);
}

#[test]
fn compatible_pairwise_protocol_keeps_existing_state() {
    let config = json!({
        "mobileRelayE2ee": {
            "protocolVersion": crate::domain::mobile_relay::support::MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
            "marker": "kept"
        },
        "paired": true
    });

    ensure_local_pairwise_protocol_compatible(&config).unwrap();

    assert_eq!(config["mobileRelayE2ee"]["marker"], "kept");
    assert_eq!(config["paired"], true);
}
