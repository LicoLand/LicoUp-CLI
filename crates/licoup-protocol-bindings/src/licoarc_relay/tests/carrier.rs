//! Versioned carrier framing and allocation-bound checks.

use super::support::*;
use sha2::{Digest, Sha256};

#[test]
fn carrier_round_trips_fixed_header_and_authenticated_content() {
    let encrypted_header = [0x31; LICOARC_ENCRYPTED_HEADER_BYTES];
    let content_ciphertext = [0x42; MIN_PADDING_BUCKET_BYTES];
    let encoded = encode_carrier(&encrypted_header, &content_ciphertext).unwrap();
    let decoded = decode_carrier(&encoded).unwrap();

    assert_eq!(decoded.encrypted_header, encrypted_header);
    assert_eq!(decoded.content_ciphertext, content_ciphertext);
    assert!(encoded.len() <= LICOARC_MAX_CIPHERTEXT_CHARS);
    assert!(!encoded.contains('='));
    assert_eq!(
        general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(encoded.as_bytes())),
        "g-jVC_51Oj1CFjTtbUkBmTk_qPz6FQz_GHDCYdabXNc"
    );
}

#[test]
fn carrier_rejects_noncanonical_version_length_and_trailing_data() {
    let encoded = encode_carrier(
        &[0x31; LICOARC_ENCRYPTED_HEADER_BYTES],
        &[0x42; MIN_PADDING_BUCKET_BYTES],
    )
    .unwrap();
    assert!(decode_carrier(&format!("{encoded}=")).is_err());

    let mut version = general_purpose::URL_SAFE_NO_PAD.decode(&encoded).unwrap();
    version[CARRIER_MAGIC.len()] = CARRIER_VERSION + 1;
    assert!(decode_carrier(&general_purpose::URL_SAFE_NO_PAD.encode(version)).is_err());

    let mut wrong_header_length = general_purpose::URL_SAFE_NO_PAD.decode(&encoded).unwrap();
    let header_length_offset = CARRIER_MAGIC.len() + 1;
    wrong_header_length[header_length_offset..header_length_offset + CARRIER_LENGTH_BYTES]
        .copy_from_slice(
            &(u32::try_from(LICOARC_ENCRYPTED_HEADER_BYTES).unwrap() - 1).to_be_bytes(),
        );
    assert!(decode_carrier(&general_purpose::URL_SAFE_NO_PAD.encode(wrong_header_length)).is_err());

    let mut trailing = general_purpose::URL_SAFE_NO_PAD.decode(&encoded).unwrap();
    trailing.push(0);
    assert!(decode_carrier(&general_purpose::URL_SAFE_NO_PAD.encode(trailing)).is_err());
}

#[test]
fn carrier_limit_fails_closed_before_allocation_or_ratchet_commit() {
    let mut largest_supported = MIN_PADDING_BUCKET_BYTES;
    loop {
        let next = if largest_supported < POWER_OF_TWO_PADDING_LIMIT_BYTES {
            largest_supported * 2
        } else {
            largest_supported + LARGE_PADDING_BUCKET_STEP_BYTES
        };
        if preflight_carrier_size(next).is_err() {
            break;
        }
        largest_supported = next;
    }

    assert!(largest_supported < MAX_PADDING_BUCKET_BYTES);
    let encoded = encode_carrier(
        &[0x31; LICOARC_ENCRYPTED_HEADER_BYTES],
        &vec![0x42; largest_supported],
    )
    .unwrap();
    assert!(encoded.len() <= LICOARC_MAX_CIPHERTEXT_CHARS);
    assert!(preflight_carrier_size(largest_supported + LARGE_PADDING_BUCKET_STEP_BYTES).is_err());
    assert!(
        encode_carrier(
            &[0x31; LICOARC_ENCRYPTED_HEADER_BYTES - 1],
            &[0x42; MIN_PADDING_BUCKET_BYTES],
        )
        .is_err()
    );
}
