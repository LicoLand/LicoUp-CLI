use serde_json::json;

use super::super::input_codec::{
    GroupCreateRequest, PublicIdentityInput, decode_base64url, encode_base64url, hex_sha256,
    parse_params,
};

#[test]
fn input_codec_requires_objects_and_canonical_bounded_base64url() {
    assert!(parse_params::<GroupCreateRequest>(&json!(null)).is_err());
    let request: GroupCreateRequest = parse_params(&json!({
        "groupIdBase64url": encode_base64url(b"codec-group")
    }))
    .unwrap();
    assert_eq!(
        decode_base64url(&request.group_id_base64url, "group id", 32).unwrap(),
        b"codec-group"
    );
    assert!(decode_base64url("YQ==", "padded", 8).is_err());
    assert!(decode_base64url(&encode_base64url(&[7; 33]), "oversized", 32).is_err());
    assert_eq!(
        hex_sha256(b"codec"),
        "a40d3dbcfe8eb987bfc1b486ec494e2d6a1a7f0388feb88909017c2a8b94b4f4"
    );
}

#[test]
fn public_identity_codec_rejects_wrong_key_lengths() {
    let input: PublicIdentityInput = parse_params(&json!({
        "endpointId": "mobile:codec",
        "identityPublicKeyBase64url": encode_base64url(&[1; 31]),
        "signingPublicKeyBase64url": encode_base64url(&[2; 32]),
        "rotationEpoch": 1
    }))
    .unwrap();
    assert!(input.to_identity().is_err());
}
