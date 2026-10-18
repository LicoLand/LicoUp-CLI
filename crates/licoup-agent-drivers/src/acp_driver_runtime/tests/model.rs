use super::*;

#[test]
fn capability_projection_uses_validated_initialize_contract() {
    let response =
        acp::validate_initialize_response(&initialize_response(true, true), INITIALIZE_REQUEST_ID)
            .unwrap();
    let capability = CapabilityProbe::from_initialize(&response);
    assert_eq!(
        capability.protocol_version,
        Some(u64::from(acp::PROTOCOL_VERSION))
    );
    assert!(capability.load_session);
    assert!(capability.resume_session);
}
