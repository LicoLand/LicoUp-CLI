use super::super::{constants::ENCODED_CHUNK_BYTES, wire::MlKemBraidMessage};

#[test]
fn strict_wire_message_rejects_unknown_and_invalid_combinations() {
    assert!(
        serde_json::from_str::<MlKemBraidMessage>(r#"{"epoch":1,"type":"None","unexpected":true}"#)
            .is_err()
    );
    assert!(serde_json::from_str::<MlKemBraidMessage>(r#"{"epoch":1,"type":"Hdr"}"#).is_err());
    assert!(serde_json::from_str::<MlKemBraidMessage>(r#"{"epoch":0,"type":"None"}"#).is_err());
    assert!(
        serde_json::from_str::<MlKemBraidMessage>(
            r#"{"epoch":1,"type":"None","data":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}"#
        )
        .is_err()
    );
    let oversized_chunk = serde_json::json!({
        "epoch": 1,
        "type": "Hdr",
        "data": "A".repeat(ENCODED_CHUNK_BYTES + 1),
    });
    assert!(serde_json::from_value::<MlKemBraidMessage>(oversized_chunk).is_err());
}
