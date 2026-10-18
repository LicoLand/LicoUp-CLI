use super::super::authorization::authorize_peer_pairwise_directory;
use super::super::claim::local_pairwise_prekey_bundle_from_config;
use super::support::endpoint_config;
use serde_json::json;
use time::OffsetDateTime;

#[test]
fn peer_authorization_rejects_descriptor_without_kt_response() {
    let config = endpoint_config();
    let bundle = local_pairwise_prekey_bundle_from_config(&config).unwrap();
    let error = authorize_peer_pairwise_directory(
        &config,
        &json!({"preKeyBundle": {}}),
        &bundle,
        OffsetDateTime::now_utc(),
    )
    .err()
    .expect("missing peer KT response must be rejected");
    assert!(
        error
            .to_string()
            .contains("peer key transparency response is missing")
    );
}
