use super::super::authorization::authorize_local_pairwise_directory;
use super::support::endpoint_config;
use crate::domain::mobile_relay::endpoint_trust::local_endpoint_state;
use crate::domain::mobile_relay::test_runtime_secret_material;
use time::OffsetDateTime;

#[test]
fn local_authorization_rejects_endpoint_without_kt_response() {
    let config = endpoint_config();
    let material = test_runtime_secret_material(stringify!(&config));
    let endpoint = local_endpoint_state(&config, &material).unwrap();
    let error = authorize_local_pairwise_directory(&config, &endpoint, OffsetDateTime::now_utc())
        .err()
        .expect("missing local KT response must be rejected");
    assert!(
        error
            .to_string()
            .contains("key transparency response is missing")
    );
}
