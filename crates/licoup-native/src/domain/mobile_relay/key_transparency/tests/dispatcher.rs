use crate::domain::mobile_relay::key_transparency::{
    SECURE_MESH_KT_NATIVE_ACTIONS, dispatch_key_transparency_action,
};
use serde_json::json;

#[test]
fn action_registry_is_closed_and_unknown_actions_fail_closed() {
    assert_eq!(SECURE_MESH_KT_NATIVE_ACTIONS.len(), 7);
    assert!(
        SECURE_MESH_KT_NATIVE_ACTIONS
            .iter()
            .all(|action| action.starts_with("secure_mesh.kt."))
    );
    let error = dispatch_key_transparency_action("secure_mesh.kt.unknown", &json!({}))
        .expect_err("unknown KT actions must fail closed");
    assert!(error.to_string().contains("unsupported"));
}
