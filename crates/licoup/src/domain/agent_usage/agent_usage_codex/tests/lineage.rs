use super::super::lineage::lineage_scope;
use std::collections::HashMap;

#[test]
fn lineage_scope_resolves_roots_and_source_fallbacks() {
    let parents = HashMap::from([
        ("child".to_string(), "parent".to_string()),
        ("parent".to_string(), "root".to_string()),
    ]);
    assert_eq!(
        lineage_scope(Some("child"), "source-key", &parents),
        "session:root"
    );
    assert_eq!(
        lineage_scope(None, "source-key", &parents),
        "source:source-key"
    );
}

#[test]
fn lineage_cycles_converge_to_a_deterministic_scope() {
    let parents = HashMap::from([
        ("beta".to_string(), "alpha".to_string()),
        ("alpha".to_string(), "beta".to_string()),
    ]);
    assert_eq!(
        lineage_scope(Some("beta"), "source-key", &parents),
        "session:alpha"
    );
}
