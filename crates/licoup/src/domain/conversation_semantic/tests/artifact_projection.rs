use serde_json::json;

use super::super::artifact_projection::artifact_from_message;

#[test]
fn artifact_projection_keeps_only_semantic_reference_fields() {
    let artifact = artifact_from_message(&json!({
        "id": "artifact-1",
        "cardTitle": "Generated report",
        "sourcePath": "relative/report.md",
        "contentHash": "digest"
    }));
    assert_eq!(artifact["layer"], "artifacts");
    assert_eq!(artifact["kind"], "document");
    assert_eq!(artifact["label"], "Generated report");
    assert_eq!(artifact["ref"], "relative/report.md");
    assert!(artifact.get("text").is_none());
}
