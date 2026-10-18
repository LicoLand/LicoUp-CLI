use base64::{Engine as _, engine::general_purpose};
use serde_json::Value;

pub(super) fn native_envelope_fixture() -> Value {
    let draft =
        licoup_protocol_bindings::licoarc_relay::LicoArcRelayEnvelopeDraft::from_contract_fields(
            &general_purpose::URL_SAFE_NO_PAD.encode([2u8; 32]),
            &general_purpose::URL_SAFE_NO_PAD.encode([1u8; 24]),
            "2030-01-01T00:00:00Z",
            256,
        )
        .expect("synthetic Lico Arc fixture metadata is valid");
    let envelope = draft
        .finish(
            &[3u8; licoup_protocol_bindings::licoarc_relay::LICOARC_ENCRYPTED_HEADER_BYTES],
            &[4u8; 256],
        )
        .expect("synthetic Lico Arc fixture carrier is valid");
    serde_json::from_str(
        &envelope
            .to_json()
            .expect("synthetic Lico Arc fixture serializes"),
    )
    .expect("synthetic Lico Arc fixture JSON parses")
}
