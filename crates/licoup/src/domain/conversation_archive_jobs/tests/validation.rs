use super::super::validation::{aggregate_validations, archive_collection_paths};
use serde_json::json;

#[test]
fn validation_projection_deduplicates_paths_and_aggregates_health() {
    let paths = archive_collection_paths(&json!({
        "archives": [
            {"collectionPath": "/local/a"},
            {"collectionPath": "/local/a"},
            {"collectionPath": "/local/b"},
        ]
    }));
    assert_eq!(paths, vec!["/local/a", "/local/b"]);

    let aggregate = aggregate_validations(&[
        json!({"collectionPath": "/local/a", "validation": {
            "healthStatus": "ok", "recordCount": 2, "rawContentBytes": 10,
            "errorCount": 0, "warningCount": 1, "issues": []
        }}),
        json!({"collectionPath": "/local/b", "validation": {
            "healthStatus": "failed", "recordCount": 1, "rawContentBytes": 5,
            "errorCount": 1, "warningCount": 0, "issues": [{"type": "corrupt"}]
        }}),
    ]);
    assert_eq!(aggregate["healthStatus"], "failed");
    assert_eq!(aggregate["recordCount"], 3);
    assert_eq!(aggregate["errorCount"], 1);
    assert_eq!(aggregate["issues"][0]["collectionPath"], "/local/b");
}
