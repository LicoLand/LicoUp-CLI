use std::ptr;

use super::super::catalog::CAPABILITY_CATALOG_JSON;
use super::super::{CapabilityCatalog, SecurityCapability, capability_catalog};

#[test]
fn canonical_catalog_is_complete_acyclic_and_dependency_ordered() {
    let catalog = capability_catalog().unwrap();
    assert_eq!(catalog.definitions().count(), SecurityCapability::COUNT);
    assert_eq!(catalog.topological_order().len(), SecurityCapability::COUNT);

    for definition in catalog.definitions() {
        let position = catalog
            .topological_order()
            .iter()
            .position(|capability| *capability == definition.capability)
            .unwrap();
        for prerequisite in &definition.prerequisites {
            let prerequisite_position = catalog
                .topological_order()
                .iter()
                .position(|capability| capability == prerequisite)
                .unwrap();
            assert!(prerequisite_position < position);
        }
    }
}

#[test]
fn canonical_catalog_digest_order_and_cache_are_deterministic() {
    let first = CapabilityCatalog::from_json(CAPABILITY_CATALOG_JSON).unwrap();
    let second = CapabilityCatalog::from_json(CAPABILITY_CATALOG_JSON).unwrap();
    assert_eq!(first.topological_order(), second.topological_order());
    assert_eq!(first.digest(), second.digest());
    assert!(ptr::eq(
        capability_catalog().unwrap(),
        capability_catalog().unwrap()
    ));
}

#[test]
fn catalog_rejects_cycles_missing_dependencies_duplicates_and_unknown_fields() {
    let cycle = r#"{
      "schemaVersion": 1,
      "capabilities": [
        {"id":"custody.os_secure_store","derived":false,"prerequisites":["custody.memory_only_ephemeral"]},
        {"id":"custody.memory_only_ephemeral","derived":false,"prerequisites":["custody.os_secure_store"]}
      ]
    }"#;
    assert!(CapabilityCatalog::from_json(cycle).is_err());

    let missing = r#"{
      "schemaVersion": 1,
      "capabilities": [
        {"id":"custody.os_secure_store","derived":false,"prerequisites":["custody.apple_keychain"]}
      ]
    }"#;
    assert!(CapabilityCatalog::from_json(missing).is_err());

    let duplicate = r#"{
      "schemaVersion": 1,
      "capabilities": [
        {"id":"custody.os_secure_store","derived":false,"prerequisites":[]},
        {"id":"custody.os_secure_store","derived":false,"prerequisites":[]}
      ]
    }"#;
    assert!(CapabilityCatalog::from_json(duplicate).is_err());

    let unknown_field = r#"{
      "schemaVersion": 1,
      "unknown": true,
      "capabilities": []
    }"#;
    assert!(CapabilityCatalog::from_json(unknown_field).is_err());
}

#[test]
fn catalog_rejects_self_edges_duplicate_edges_and_retired_entry_fields() {
    let self_edge = r#"{
      "schemaVersion": 1,
      "capabilities": [
        {"id":"custody.os_secure_store","derived":false,"prerequisites":["custody.os_secure_store"]}
      ]
    }"#;
    assert!(CapabilityCatalog::from_json(self_edge).is_err());

    let duplicate_edge = r#"{
      "schemaVersion": 1,
      "capabilities": [
        {"id":"custody.os_secure_store","derived":false,"prerequisites":[]},
        {"id":"custody.memory_only_ephemeral","derived":false,"prerequisites":["custody.os_secure_store","custody.os_secure_store"]}
      ]
    }"#;
    assert!(CapabilityCatalog::from_json(duplicate_edge).is_err());

    let retired_fields = r#"{
      "schemaVersion": 1,
      "capabilities": [
        {"id":"custody.memory_only_ephemeral","scope":"local_custody","mandatory":true,"derived":false,"prerequisites":[]}
      ]
    }"#;
    assert!(CapabilityCatalog::from_json(retired_fields).is_err());
}

#[test]
fn catalog_input_is_bounded() {
    let oversized = " ".repeat(256 * 1024 + 1);
    assert!(CapabilityCatalog::from_json(&oversized).is_err());
}
