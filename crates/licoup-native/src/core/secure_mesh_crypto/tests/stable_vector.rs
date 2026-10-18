use super::test_support::*;

#[test]
fn secure_mesh_content_crypto_has_stable_vectors_for_all_payload_kinds() {
    let context = context_fixture();
    #[allow(dead_code)]
    struct ContentCryptoStableVector {
        label: &'static str,
        payload: SecureMeshPlaintext,
        nonce: [u8; CONTENT_NONCE_LEN],
        encrypted_header: &'static str,
        ciphertext: &'static str,
        ciphertext_sha256: &'static str,
        ciphertext_size: usize,
    }
    let vectors = [
        ContentCryptoStableVector {
            label: "command",
            payload: SecureMeshPlaintext::new(
                SecureMeshPayloadKind::Command,
                br#"{"op":"agent.message.send"}"#,
            ),
            nonce: [11u8; CONTENT_NONCE_LEN],
            encrypted_header: "TENPU00tSERSLXYxCwsLCwsLCwsLCwsLcKwToeqmQn2JWKBETud3qIoCc9UF9ZjKgrcNH5XOQuQ",
            ciphertext: "-ar9u2Fl3ad9Jlu8ULlk-1qFuHQ1ADwOy6-NmMCE-bSMpoHNZ1QUhKDhoj7kNyrT1Ovw1vZWl2VpnKSPy8d2UJp6WdPiGeljxpjAWNopTCzcSeA-TmtzunzIGpwuW2zV21VngG1U11eFEBAMhqhp1Z6NDag4ejnvsQ",
            ciphertext_sha256: "sha256:65cac3b77fbf30e5e0705a544d9fc0e67e964f4ed732b1c52761d79763e61fbc",
            ciphertext_size: 256,
        },
        ContentCryptoStableVector {
            label: "result",
            payload: SecureMeshPlaintext::new(
                SecureMeshPayloadKind::ResultPayload,
                br#"{"ok":true}"#,
            ),
            nonce: [12u8; CONTENT_NONCE_LEN],
            encrypted_header: "TENPU00tSERSLXYxDAwMDAwMDAwMDAwMwZzMnPPsv14bO5HpqSRapa1-cJxI8ZquUu8nc3nbd74",
            ciphertext: "1rFim7RvXCwMI9sriRbvIbQv9WFIl4oGmJ1gna2PPRCGkoHNKBAi9mEvzhWqft-pJjoPz41mTONOrZj5fLn3t5KyBFyGk-j7KQBTJZfJdMROBU-bLr4dBS2-PHuuzYi85J_j8A",
            ciphertext_sha256: "sha256:cb5f9c85a8f6f08f43999a1595fe2e25031330b94bdec5e188635747b96e9b5a",
            ciphertext_size: 256,
        },
        ContentCryptoStableVector {
            label: "error",
            payload: SecureMeshPlaintext::new(SecureMeshPayloadKind::Error, b"permission denied"),
            nonce: [13u8; CONTENT_NONCE_LEN],
            encrypted_header: "TENPU00tSERSLXYxDQ0NDQ0NDQ0NDQ0ND8MAujGATexg6zDuqf8oeCcaG6cFnmPDKRr590IHyOU",
            ciphertext: "wQLaYL3c7UeoiE5DNukuq-Dj_piq8PtcunwOZZYWHQ8AJcYYYIk4OC_NP8lhCt5nYK2rO_G72kuhFu3O6WHAEk5xJRoabmn0MQA_0vJ4dBeSlybZanmN1WmVhVl3uOZY6CJBGBEcrMtBuA",
            ciphertext_sha256: "sha256:bce695fc0a6997f26ce28c70c6f54b921d07d6faa69d042b10e18ef503db4d71",
            ciphertext_size: 256,
        },
        ContentCryptoStableVector {
            label: "file_chunk",
            payload: SecureMeshPlaintext::new(SecureMeshPayloadKind::FileChunk, b"file-bytes")
                .with_content_type("application/octet-stream"),
            nonce: [14u8; CONTENT_NONCE_LEN],
            encrypted_header: "TENPU00tSERSLXYxDg4ODg4ODg4ODg4Ob59u8LfuS_EwGDCim5B62Ng4U2563YH_zjeKDPgdOnc",
            ciphertext: "UzCiIRVjxbR-EeYrzWLeo1KcpsSvnu_xMcEpZkMVv0bkX8TRcYH0q5gDMYL5sx-vuEvl_qhXhf7LaDnNxptEKR_JVoAwDYS3ojf6poMeEza08p3qxDo6iysCzs5sutdPeh-R1r8JX9qjFP6PQny5sSx3AssTqCDTC5u07Ojy9Q",
            ciphertext_sha256: "sha256:c85ec3b22a383aa6e48e432ca6aec8e9f71e57dbe2bccf14ec246a10a25e9564",
            ciphertext_size: 256,
        },
        ContentCryptoStableVector {
            label: "file_manifest",
            payload: SecureMeshPlaintext::new(
                SecureMeshPayloadKind::FileManifest,
                br#"{"name":"redacted.bin","chunks":1}"#,
            )
            .with_content_type("application/json"),
            nonce: [15u8; CONTENT_NONCE_LEN],
            encrypted_header: "TENPU00tSERSLXYxDw8PDw8PDw8PDw8P9qbXGiRNRyzma8lS01pHypVVSuEQHCJWH9EwneJToVc",
            ciphertext: "p5Fc0jS1RBf7QsOt3EFGZ8-ycm_J9XBv4-93a_JfJnR5gNVStZe6azbGF8z5HRefwEg0K3EIV4ACwtJvn-5PrJulOb-e2HV4Jdo6DeVxeK8eT5Be6h8STi3Tf9ErMeY_QSjeB7CVlmY97DqAIxuAWT7chvW0SRVCgsNUb1xdGMYHa6RfB3WpNmDmuxfbFNE",
            ciphertext_sha256: "sha256:52cc1a41520f24e0b90da6ec7d890934adbe4b0da0cf46a0e7dff52b751f9991",
            ciphertext_size: 256,
        },
        ContentCryptoStableVector {
            label: "service_action",
            payload: SecureMeshPlaintext::new(
                SecureMeshPayloadKind::ServiceAction,
                br#"{"actionKind":"message_delete","messageHash":"sha256:redacted"}"#,
            )
            .with_content_type("application/json"),
            nonce: [16u8; CONTENT_NONCE_LEN],
            encrypted_header: "TENPU00tSERSLXYxEBAQEBAQEBAQEBAQ83qdCozqb7tFu48wUbNabVnxXqkVh3vrK7QGFX13GrM",
            ciphertext: "lsXkoS6g6VIdBbuWcqE_cX21dd2YVLQZlZDDFc4Rp-75DeqoqPJZkIqfjub6cJjpV0ags0gAG7yyJV6LmE99C-D0kRcnR3_kPszFL1xBcoBLejNRUR3wk-NQ5oM2drUamnCHZoJyy3l0bdArmbC8kK1FnMfylJl7KncpSCvZ5k3lFMWHU0SjpssXnjfEm0oiX206_rhW_suQLrt9brF6r5WYt6Amk8JPP7CQ8g",
            ciphertext_sha256: "sha256:41a32319982ec9a6c0a9ddc2bac22bdbf33a4338ba18000010405aa5324de091",
            ciphertext_size: 256,
        },
        ContentCryptoStableVector {
            label: "typing_indicator",
            payload: SecureMeshPlaintext::new(
                SecureMeshPayloadKind::TypingIndicator,
                br#"{"typingState":"started"}"#,
            )
            .with_content_type("application/json"),
            nonce: [17u8; CONTENT_NONCE_LEN],
            encrypted_header: "TENPU00tSERSLXYxERERERERERERERER5cw-KSL-uP4ANB8GlQ-8XDcfHCgCnTXPsu6PwaF5Snc",
            ciphertext: "FYxeYCydlCadYK6SlgAP6oTjlPzPmOD8PaS3e5Z76Ai0nTDk61Qt9eHNFfDO3noCbzpBw4eBvpi4pwt3bg96nodSxzptRNleo9OWj7mTovpOyRP-5AmSXBi9VVc2Jj9PiKIvUIflPwsms2i2b8gekmObUNZYoRzSq_XpCoEyZTgk5h6A3Fk",
            ciphertext_sha256: "sha256:70561a8c3ecf7553b5f9881df46bfebd1d8e15fc12aefe4d6740774b76c42aca",
            ciphertext_size: 256,
        },
        ContentCryptoStableVector {
            label: "read_receipt",
            payload: SecureMeshPlaintext::new(
                SecureMeshPayloadKind::ReadReceipt,
                br#"{"readUpToMessageDigest":"sha256:redacted"}"#,
            )
            .with_content_type("application/json"),
            nonce: [18u8; CONTENT_NONCE_LEN],
            encrypted_header: "TENPU00tSERSLXYxEhISEhISEhISEhISKX6DyVnbjc-Tb0UA-5jHUg1zK6m3e1naHJsF42eSpvM",
            ciphertext: "XYzZ45FRETlpANS-Rwfh3-pcmvIaxXepYScebTtgIzMi9xiQTByWXc0786CCI_qbdJi3TKeGaxB0HoYWaZpZGXjAdFBCNTCIOXiezxdm-7lY8e9blANraywaO5kjzsbr9VK2sWUswWgxB4LdE9nsrBKfITfQRRv9an_nuWoTDVtx1fIQRdJyo0JDNIJPcwKab_TKcOrxw1k",
            ciphertext_sha256: "sha256:3420a9ad4f7e316aa69a6ad76d1565e43ef64e2dcb2d98e0f19bff64f7b574ba",
            ciphertext_size: 256,
        },
    ];
    assert_eq!(vectors.len(), 8);
    for vector in vectors {
        let candidate =
            seal_payload_with_nonce(&key_fixture(42), &context, &vector.payload, vector.nonce)
                .unwrap();
        assert_eq!(
            candidate.encrypted_header, vector.encrypted_header,
            "{} encrypted header vector changed",
            vector.label
        );
        let candidate_ciphertext_sha256 = format!(
            "sha256:{}",
            Sha256::digest(candidate.ciphertext.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        assert_eq!(
            candidate_ciphertext_sha256, vector.ciphertext_sha256,
            "{} ciphertext digest vector changed",
            vector.label
        );
        assert_eq!(
            candidate.ciphertext_size, vector.ciphertext_size,
            "{} ciphertext size vector changed",
            vector.label
        );
        let opened =
            open_payload(&key_fixture(42), &context, &candidate, vector.payload.kind).unwrap();
        assert_eq!(opened.kind, vector.payload.kind, "{} kind", vector.label);
        assert_eq!(opened.body, vector.payload.body, "{} body", vector.label);
        assert_eq!(
            opened.content_type, vector.payload.content_type,
            "{} content type",
            vector.label
        );
    }
}
