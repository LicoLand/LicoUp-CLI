use licoup_foundation::platform::file_security::ensure_private_dir;
use serde_json::json;

use super::support::TestRoot;

#[test]
fn bounded_private_json_round_trip_is_atomic() {
    let root = TestRoot::new("serialization");
    let state = root.path().join("state");
    ensure_private_dir(&state).unwrap();
    let path = state.join("document.json");
    crate::serialization::atomic_write_json(&path, &json!({"ok": true}), 1024).unwrap();

    let document = crate::serialization::read_json_or_default(&path, 1024, || json!({})).unwrap();
    assert_eq!(document["ok"], true);
    assert!(crate::serialization::read_json_or_default(&path, 2, || json!({})).is_err());
}

#[test]
fn identifiers_and_hashes_are_deterministic_and_bounded() {
    assert_eq!(
        crate::serialization::sanitize_id(" ../Open Code "),
        "Open-Code"
    );
    assert_eq!(crate::serialization::sanitize_id("///"), "item");
    assert_eq!(
        crate::serialization::sanitize_id(&"a".repeat(100)).len(),
        64
    );
    assert_eq!(
        crate::serialization::hash_text("same"),
        crate::serialization::hash_text("same")
    );
    assert_ne!(
        crate::serialization::hash_text("same"),
        crate::serialization::hash_text("different")
    );
}
