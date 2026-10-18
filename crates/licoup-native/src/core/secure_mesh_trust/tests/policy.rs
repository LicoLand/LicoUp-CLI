use super::super::evaluate_device_trust_policy_json;
use super::support::{identity_fixture, identity_json};
use serde_json::json;

#[test]
fn secure_mesh_device_trust_policy_json_treats_caller_verified_as_advisory_and_blocks_key_change() {
    let (_alice_key, alice) = identity_fixture("desktop_gui:alice");
    let (_replacement_key, mut replacement) = identity_fixture("desktop_gui:alice");
    replacement.endpoint_id = alice.endpoint_id.clone();
    let trusted = evaluate_device_trust_policy_json(&json!({
        "identity": identity_json(&alice),
        "trustState": "verified",
        "requireVerifiedDevice": true
    }))
    .unwrap();
    assert_eq!(trusted["requestedTrustState"], "verified");
    assert_eq!(trusted["trustState"], "unverified");
    assert_eq!(trusted["decision"]["allowedForPrekey"], false);
    assert_eq!(trusted["decision"]["allowedForHighRiskCommand"], false);
    assert_eq!(trusted["decision"]["code"], "verification_required");
    assert_eq!(
        trusted["policy"]["positiveAuthorizationSource"],
        "persisted_local_signed_trust_record_only"
    );

    let changed = evaluate_device_trust_policy_json(&json!({
        "identity": identity_json(&replacement),
        "previousIdentity": identity_json(&alice),
        "trustState": "verified",
        "requireVerifiedDevice": true
    }))
    .unwrap();
    assert_eq!(changed["keyChangeDetected"], true);
    assert_eq!(changed["trustState"], "key_changed");
    assert_eq!(changed["decision"]["allowedForPrekey"], false);
    assert_eq!(changed["decision"]["code"], "identity_key_changed");
}
