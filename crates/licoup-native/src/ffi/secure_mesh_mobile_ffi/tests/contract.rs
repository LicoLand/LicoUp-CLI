use super::test_support::MOBILE_RELAY_NATIVE_ACTIONS;
use serde_json::Value;
use std::collections::BTreeSet;

#[test]
fn secure_mesh_contract_actions_and_schemas_are_drift_free() {
    let schema = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../schemas/client_bridge/secure_mesh.json"
    ));
    let schema: Value = serde_json::from_str(&schema)
        .unwrap_or_else(|error| panic!("secure mesh schema must be valid json: {error}"));

    let schema_actions = schema
        .get("actions")
        .and_then(Value::as_array)
        .expect("schema must define actions")
        .iter()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let native_actions = MOBILE_RELAY_NATIVE_ACTIONS
        .iter()
        .copied()
        .filter(|action| action.starts_with("secure_mesh."))
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    assert_eq!(
        BTreeSet::from_iter(schema_actions),
        BTreeSet::from_iter(native_actions)
    );
}

#[test]
fn secure_mesh_contract_limits_match_schema() {
    let schema = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../schemas/client_bridge/secure_mesh.json"
    ));
    let schema: Value = serde_json::from_str(&schema)
        .unwrap_or_else(|error| panic!("secure mesh schema must be valid json: {error}"));

    let max_request_bytes = schema
        .get("maxRequestBytes")
        .and_then(Value::as_u64)
        .expect("maxRequestBytes") as usize;
    let max_depth = schema
        .get("maxDepth")
        .and_then(Value::as_u64)
        .expect("maxDepth") as usize;
    let max_nodes = schema
        .get("maxNodes")
        .and_then(Value::as_u64)
        .expect("maxNodes") as usize;
    let max_string_bytes = schema
        .get("maxStringBytes")
        .and_then(Value::as_u64)
        .expect("maxStringBytes") as usize;

    use super::test_support::{
        MAX_FFI_JSON_DEPTH, MAX_FFI_JSON_NODES, MAX_FFI_OBJECT_FIELDS, MAX_FFI_REQUEST_BYTES,
        MAX_FFI_STRING_BYTES,
    };

    assert_eq!(max_request_bytes, MAX_FFI_REQUEST_BYTES);
    assert_eq!(max_depth, MAX_FFI_JSON_DEPTH);
    assert_eq!(max_nodes, MAX_FFI_JSON_NODES);
    assert_eq!(max_string_bytes, MAX_FFI_STRING_BYTES);
    assert_eq!(
        schema
            .get("maxCollectionEntries")
            .and_then(Value::as_u64)
            .expect("maxCollectionEntries") as usize,
        MAX_FFI_OBJECT_FIELDS
    );
}

#[test]
fn secure_mesh_contract_typed_api_symbols_are_present() {
    let generated = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/ffi/generated/secure_mesh.rs"
    ));
    let dispatch_router = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/ffi/secure_mesh_mobile_ffi/dispatch_router.rs"
    ));

    for symbol in [
        "SecureMeshAction",
        "SecureMeshRequest",
        "SecureMeshResult",
        "SecureMeshFailure",
    ] {
        assert!(
            generated.contains(symbol),
            "generated secure mesh contract must expose {symbol}"
        );
    }

    for symbol in ["dispatch_request", "SecureMeshRequest", "SecureMeshResult"] {
        assert!(
            dispatch_router.contains(symbol),
            "dispatch router should route through generated typed request/response API"
        );
    }
}

#[test]
fn secure_mesh_contract_failure_codes_cover_secret_and_unsupported_inputs() {
    let schema = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../schemas/client_bridge/secure_mesh.json"
    ));
    let schema: Value = serde_json::from_str(&schema)
        .unwrap_or_else(|error| panic!("secure mesh schema must be valid json: {error}"));
    let generated = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/ffi/generated/secure_mesh.rs"
    ));

    let codes = schema
        .get("failureCodes")
        .and_then(Value::as_array)
        .expect("failure codes")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();

    for code in codes {
        assert!(
            generated.contains(code),
            "generated contract must expose failure code {code}"
        );
    }

    use crate::ffi::generated::secure_mesh::{
        SecureMeshAction, SecureMeshFailureCode, SecureMeshRequest,
    };
    use serde_json::json;

    let request = SecureMeshRequest::new(SecureMeshAction::SecureMeshStatus, json!({}))
        .expect("bounded status request");
    let result = super::super::dispatch_request(&request, "native_test_forbidden_secret_material")
        .expect("typed status dispatch");
    assert_eq!(result.value()["ok"], true);

    let secret = SecureMeshRequest::new(
        SecureMeshAction::KtStatus,
        json!({"privateKey": "unit-test-secret"}),
    )
    .expect_err("secret-bearing payload must fail before dispatch");
    assert_eq!(secret.code, SecureMeshFailureCode::ForbiddenSecretMaterial);
    assert!(
        !serde_json::to_string(&secret)
            .expect("failure serializes")
            .contains("unit-test-secret")
    );

    let unsupported = SecureMeshRequest::from_value(json!({
        "action": "secure_mesh.unknown",
        "params": {}
    }))
    .expect_err("unknown action must fail closed");
    assert_eq!(unsupported.code, SecureMeshFailureCode::UnsupportedAction);
}
