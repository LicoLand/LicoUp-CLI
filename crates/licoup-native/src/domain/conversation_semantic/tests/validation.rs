use std::path::PathBuf;

use serde_json::json;

use super::super::io::load_and_validate_fixture;
use super::super::validation::validate_semantic_conversation;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/contracts/client/fixtures/semantic-conversation")
}

#[test]
fn contract_fixtures_pass_schema_validation() {
    for name in ["complete-layers.json", "metadata-filtered.json"] {
        let path = fixture_dir().join(name);
        load_and_validate_fixture(&path)
            .unwrap_or_else(|error| panic!("fixture {name} failed: {error}"));
    }
}

#[test]
fn validator_rejects_invalid_constants_and_default_layer_leakage() {
    let mut invalid_kind =
        load_and_validate_fixture(&fixture_dir().join("complete-layers.json")).expect("fixture");
    invalid_kind["kind"] = json!("other");
    assert!(validate_semantic_conversation(&invalid_kind).is_err());

    let mut leakage =
        load_and_validate_fixture(&fixture_dir().join("complete-layers.json")).expect("fixture");
    leakage["thread"][0]["text"] = json!(format!(
        "see {}/{}",
        concat!("/", "Users"),
        "person/private"
    ));
    assert!(validate_semantic_conversation(&leakage).is_err());
}
