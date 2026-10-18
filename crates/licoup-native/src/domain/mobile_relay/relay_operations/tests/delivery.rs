use super::super::delivery::relay_envelope_from_delivery;
use serde_json::json;

#[test]
fn delivery_conversion_rejects_non_lico_arc_shapes() {
    assert!(relay_envelope_from_delivery(&json!({"unexpected": "field"})).is_err());
}
