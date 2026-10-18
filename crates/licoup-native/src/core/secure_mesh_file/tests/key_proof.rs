use super::support::*;

#[test]
fn file_root_key_is_random_redacted_and_hkdf_domains_are_disjoint() {
    let first = FileRootKey::generate();
    let second = FileRootKey::generate();
    assert_ne!(first.as_bytes(), second.as_bytes());
    assert_eq!(format!("{first:?}"), "FileRootKey([redacted])");

    let manifest = manifest_fixture();
    let context = context_fixture("domain", "msg_domain", &manifest);
    let manifest_aad = file_authenticated_data(
        &context,
        FILE_AAD_MANIFEST_PURPOSE,
        None,
        context.file_hash(),
    )
    .unwrap();
    let chunk_zero_aad = file_authenticated_data(
        &context,
        FILE_AAD_CHUNK_PURPOSE,
        Some(0),
        context.file_hash(),
    )
    .unwrap();
    let chunk_one_aad = file_authenticated_data(
        &context,
        FILE_AAD_CHUNK_PURPOSE,
        Some(1),
        context.file_hash(),
    )
    .unwrap();
    let chunk_hash_aad = file_authenticated_data(
        &context,
        FILE_AAD_CHUNK_HASH_PURPOSE,
        Some(0),
        context.file_hash(),
    )
    .unwrap();
    let receipt_hash = hash_bytes(b"receipt-ciphertext");
    let receipt_aad =
        file_authenticated_data(&context, FILE_AAD_RECEIPT_PURPOSE, Some(0), &receipt_hash)
            .unwrap();
    let key_wrap_aad = file_authenticated_data(
        &context,
        FILE_AAD_KEY_WRAP_PURPOSE,
        None,
        context.file_hash(),
    )
    .unwrap();
    let keys = [
        derive_file_key(first.as_bytes(), FILE_HKDF_MANIFEST_DOMAIN, &manifest_aad).unwrap(),
        derive_file_key(first.as_bytes(), FILE_HKDF_CHUNK_DOMAIN, &chunk_zero_aad).unwrap(),
        derive_file_key(first.as_bytes(), FILE_HKDF_CHUNK_DOMAIN, &chunk_one_aad).unwrap(),
        derive_file_key(
            first.as_bytes(),
            FILE_HKDF_CHUNK_HASH_DOMAIN,
            &chunk_hash_aad,
        )
        .unwrap(),
        derive_file_key(first.as_bytes(), FILE_HKDF_RECEIPT_DOMAIN, &receipt_aad).unwrap(),
        derive_file_key(first.as_bytes(), FILE_HKDF_KEY_WRAP_DOMAIN, &key_wrap_aad).unwrap(),
    ];
    assert_eq!(
        keys.into_iter()
            .map(|key| key.as_ref().to_vec())
            .collect::<HashSet<_>>()
            .len(),
        6
    );
}

#[test]
fn pairwise_file_key_envelopes_are_per_device_and_reject_duplicate_recipients() {
    let root_key = FileRootKey::from_bytes([0x31; FILE_ROOT_KEY_BYTES]);
    let manifest = manifest_fixture();
    let file_hash = hash_bytes(b"pairwise-multi-device-file");
    let first_context = pairwise_context_fixture(
        "pairwise_key_one",
        "pairwise_key_one",
        &manifest,
        "sender:endpoint",
        "recipient:one",
        "pairwise:session:one",
        &file_hash,
        1_800_000_000,
    );
    let second_context = pairwise_context_fixture(
        "pairwise_key_two",
        "pairwise_key_two",
        &manifest,
        "sender:endpoint",
        "recipient:two",
        "pairwise:session:two",
        &file_hash,
        1_800_000_000,
    );
    let first_secret = FileKeyWrapSecret::from_bytes([0x32; FILE_KEY_WRAP_SECRET_BYTES]);
    let second_secret = FileKeyWrapSecret::from_bytes([0x33; FILE_KEY_WRAP_SECRET_BYTES]);
    let envelopes = seal_file_root_key_for_pairwise_devices(
        &root_key,
        [
            (&first_secret, &first_context),
            (&second_secret, &second_context),
        ],
    )
    .unwrap();
    assert_eq!(envelopes.len(), 2);
    assert_ne!(envelopes[0].ciphertext, envelopes[1].ciphertext);
    let first_opened = open_file_root_key_for_pairwise_device(
        &envelopes[0],
        &first_secret,
        &first_context,
        1_700_000_000,
    )
    .unwrap();
    let second_opened = open_file_root_key_for_pairwise_device(
        &envelopes[1],
        &second_secret,
        &second_context,
        1_700_000_000,
    )
    .unwrap();
    assert_eq!(first_opened.as_bytes(), root_key.as_bytes());
    assert_eq!(second_opened.as_bytes(), root_key.as_bytes());
    assert!(
        open_file_root_key_for_pairwise_device(
            &envelopes[0],
            &second_secret,
            &second_context,
            1_700_000_000,
        )
        .is_err()
    );

    let duplicate_context = pairwise_context_fixture(
        "pairwise_key_duplicate",
        "pairwise_key_duplicate",
        &manifest,
        "sender:endpoint",
        "recipient:one",
        "pairwise:session:duplicate",
        &file_hash,
        1_800_000_000,
    );
    assert!(
        seal_file_root_key_for_pairwise_devices(
            &root_key,
            [
                (&first_secret, &first_context),
                (&second_secret, &duplicate_context),
            ],
        )
        .unwrap_err()
        .to_string()
        .contains("duplicated")
    );
}

#[test]
fn mls_file_key_envelope_rejects_wrong_epoch_context_expiry_tamper_and_old_format() {
    let root_key = FileRootKey::from_bytes([0x41; FILE_ROOT_KEY_BYTES]);
    let exporter_secret = FileKeyWrapSecret::from_bytes([0x42; FILE_KEY_WRAP_SECRET_BYTES]);
    let manifest = manifest_fixture();
    let file_hash = hash_bytes(b"mls-epoch-file");
    let context = mls_context_fixture(
        "mls_key",
        "mls_key",
        &manifest,
        "sender:endpoint",
        "recipient:endpoint",
        "mls:session",
        &file_hash,
        "mls:group",
        7,
        1_800_000_000,
    );
    let envelope = seal_file_root_key_for_mls_epoch(&root_key, &exporter_secret, &context).unwrap();
    let opened =
        open_file_root_key_for_mls_epoch(&envelope, &exporter_secret, &context, 7, 1_700_000_000)
            .unwrap();
    assert_eq!(opened.as_bytes(), root_key.as_bytes());
    assert!(
        open_file_root_key_for_mls_epoch(&envelope, &exporter_secret, &context, 8, 1_700_000_000,)
            .is_err()
    );
    assert!(
        open_file_root_key_for_mls_epoch(&envelope, &exporter_secret, &context, 7, 1_800_000_001,)
            .is_err()
    );

    let wrong_context = mls_context_fixture(
        "mls_key",
        "mls_key",
        &manifest,
        "sender:endpoint",
        "recipient:attacker",
        "mls:session",
        &file_hash,
        "mls:group",
        7,
        1_800_000_000,
    );
    assert!(
        open_file_root_key_for_mls_epoch(
            &envelope,
            &exporter_secret,
            &wrong_context,
            7,
            1_700_000_000,
        )
        .is_err()
    );

    let mut tampered: Value = serde_json::from_str(&envelope.to_json().unwrap()).unwrap();
    let mut ciphertext = general_purpose::URL_SAFE_NO_PAD
        .decode(tampered["ciphertext"].as_str().unwrap())
        .unwrap();
    ciphertext[0] ^= 1;
    tampered["ciphertext"] = json!(general_purpose::URL_SAFE_NO_PAD.encode(ciphertext));
    let tampered = FileKeyEnvelope::from_json(&serde_json::to_string(&tampered).unwrap()).unwrap();
    assert!(
        open_file_root_key_for_mls_epoch(&tampered, &exporter_secret, &context, 7, 1_700_000_000,)
            .is_err()
    );
    assert!(FileKeyEnvelope::from_json(r#"{"fileKeyBytes":[1,2,3]}"#).is_err());
    let mut unknown: Value = serde_json::from_str(&envelope.to_json().unwrap()).unwrap();
    unknown["legacyFileKey"] = json!("forbidden");
    assert!(FileKeyEnvelope::from_json(&serde_json::to_string(&unknown).unwrap()).is_err());
}

#[test]
fn file_receipt_authentication_rejects_hash_tag_context_and_expiry_tampering() {
    let root_key = key_fixture();
    let manifest = manifest_fixture();
    let context = context_fixture("receipt_chunk", "receipt_chunk", &manifest);
    let chunk = SecureMeshFileChunk {
        file_id: manifest.file_id.clone(),
        chunk_index: 0,
        bytes: vec![0x51; manifest.chunk_size as usize],
    };
    let encrypted = seal_file_chunk(&root_key, &context, &chunk).unwrap();
    let receipt =
        authenticate_file_chunk_receipt(&root_key, &context, &encrypted, 1_700_000_000).unwrap();
    verify_file_chunk_receipt(&root_key, &context, &receipt, 1_700_000_000).unwrap();

    let mut tampered_hash = receipt.clone();
    tampered_hash.ciphertext_hash = hash_bytes(b"tampered-ciphertext");
    assert!(
        verify_file_chunk_receipt(&root_key, &context, &tampered_hash, 1_700_000_000,).is_err()
    );
    let mut tampered_tag = receipt.clone();
    tampered_tag.authentication_tag = general_purpose::URL_SAFE_NO_PAD.encode([0u8; 32]);
    assert!(verify_file_chunk_receipt(&root_key, &context, &tampered_tag, 1_700_000_000,).is_err());
    let wrong_context = pairwise_context_fixture(
        "receipt_chunk",
        "receipt_chunk",
        &manifest,
        "desktop_gui:alpha",
        "mobile:attacker",
        "file_session_test",
        context.file_hash(),
        1_800_000_000,
    );
    assert!(
        verify_file_chunk_receipt(&root_key, &wrong_context, &receipt, 1_700_000_000,).is_err()
    );
    assert!(verify_file_chunk_receipt(&root_key, &context, &receipt, 1_800_000_001).is_err());
}

#[test]
fn file_payload_aad_rejects_every_bound_context_dimension_and_metadata_tamper() {
    let root_key = key_fixture();
    let manifest = manifest_fixture();
    let file_hash = hash_bytes(b"canonical-complete-file-hash");
    let context = pairwise_context_fixture(
        "aad_bound",
        "aad_bound",
        &manifest,
        "sender:endpoint",
        "recipient:endpoint",
        "pairwise:session",
        &file_hash,
        1_800_000_000,
    );
    let chunk = SecureMeshFileChunk {
        file_id: manifest.file_id.clone(),
        chunk_index: 0,
        bytes: vec![0x61; manifest.chunk_size as usize],
    };
    let encrypted_manifest = seal_file_manifest(&root_key, &context, &manifest).unwrap();
    let encrypted_chunk = seal_file_chunk(&root_key, &context, &chunk).unwrap();

    let context_variants = [
        pairwise_context_fixture(
            "aad_bound",
            "aad_bound",
            &manifest,
            "sender:attacker",
            "recipient:endpoint",
            "pairwise:session",
            &file_hash,
            1_800_000_000,
        ),
        pairwise_context_fixture(
            "aad_bound",
            "aad_bound",
            &manifest,
            "sender:endpoint",
            "recipient:attacker",
            "pairwise:session",
            &file_hash,
            1_800_000_000,
        ),
        pairwise_context_fixture(
            "aad_bound",
            "aad_bound",
            &manifest,
            "sender:endpoint",
            "recipient:endpoint",
            "pairwise:session:wrong",
            &file_hash,
            1_800_000_000,
        ),
        pairwise_context_fixture(
            "aad_bound",
            "aad_bound",
            &manifest,
            "sender:endpoint",
            "recipient:endpoint",
            "pairwise:session",
            &hash_bytes(b"wrong-file-hash"),
            1_800_000_000,
        ),
        pairwise_context_fixture(
            "aad_bound",
            "aad_bound",
            &manifest,
            "sender:endpoint",
            "recipient:endpoint",
            "pairwise:session",
            &file_hash,
            1_800_000_001,
        ),
    ];
    for wrong_context in context_variants {
        assert!(open_file_manifest(&root_key, &wrong_context, &encrypted_manifest).is_err());
        assert!(open_file_chunk(&root_key, &wrong_context, &encrypted_chunk).is_err());
    }

    let mut wrong_file_manifest = manifest.clone();
    wrong_file_manifest.file_id = "wrong-file-id".to_string();
    let wrong_file_context = pairwise_context_fixture(
        "aad_bound",
        "aad_bound",
        &wrong_file_manifest,
        "sender:endpoint",
        "recipient:endpoint",
        "pairwise:session",
        &file_hash,
        1_800_000_000,
    );
    assert!(open_file_manifest(&root_key, &wrong_file_context, &encrypted_manifest).is_err());

    let mut wrong_count_manifest = manifest.clone();
    wrong_count_manifest.chunk_count -= 1;
    let wrong_count_context = pairwise_context_fixture(
        "aad_bound",
        "aad_bound",
        &wrong_count_manifest,
        "sender:endpoint",
        "recipient:endpoint",
        "pairwise:session",
        &file_hash,
        1_800_000_000,
    );
    assert!(open_file_manifest(&root_key, &wrong_count_context, &encrypted_manifest).is_err());

    let mut tampered_manifest = encrypted_manifest.clone();
    tampered_manifest.file_aad_digest = general_purpose::URL_SAFE_NO_PAD.encode([0x62u8; 32]);
    assert!(open_file_manifest(&root_key, &context, &tampered_manifest).is_err());
    let mut wrong_suite_chunk = encrypted_chunk.clone();
    wrong_suite_chunk.file_key_suite = "removed-file-key-suite-v1".to_string();
    assert!(open_file_chunk(&root_key, &context, &wrong_suite_chunk).is_err());
    let mut wrong_index_chunk = encrypted_chunk;
    wrong_index_chunk.chunk_index = 1;
    assert!(open_file_chunk(&root_key, &context, &wrong_index_chunk).is_err());
}

#[test]
fn secure_mesh_file_key_wraps_through_pairwise_session_before_file_open() {
    let (mut alice_session, mut bob_session) = pairwise_file_sessions();
    let file_key_bytes = [41u8; 32];
    let file_key = FileRootKey::from_bytes(file_key_bytes);
    let wrap_secret = FileKeyWrapSecret::from_bytes([42u8; 32]);
    let manifest = SecureMeshFileManifest {
        file_id: "file-key-wrap-integration".to_string(),
        file_name: "pairwise-wrapped-file.txt".to_string(),
        mime_type: "text/plain".to_string(),
        relative_path: "pairwise/wrapped".to_string(),
        total_size: 25,
        chunk_size: 25,
        chunk_count: 1,
    };
    let chunk = SecureMeshFileChunk {
        file_id: manifest.file_id.clone(),
        chunk_index: 0,
        bytes: b"pairwise sealed file bytes".to_vec(),
    };
    let file_hash = hash_bytes(&chunk.bytes);
    let manifest_context = pairwise_file_protection_context(
        &alice_session,
        "env_file_manifest_pairwise_wrapped",
        "msg_file_manifest_pairwise_wrapped",
        &manifest,
        &file_hash,
    );
    let chunk_context = pairwise_file_protection_context(
        &alice_session,
        "env_file_chunk_pairwise_wrapped",
        "msg_file_chunk_pairwise_wrapped",
        &manifest,
        &file_hash,
    );
    let encrypted_manifest = seal_file_manifest(&file_key, &manifest_context, &manifest).unwrap();
    let encrypted_chunk = seal_file_chunk(&file_key, &chunk_context, &chunk).unwrap();

    let key_wrap_context = pairwise_file_protection_context(
        &alice_session,
        "env_file_key_pairwise_wrapped",
        "msg_file_key_pairwise_wrapped",
        &manifest,
        &file_hash,
    );
    let wrapped_file_key =
        seal_file_root_key_for_pairwise_device(&file_key, &wrap_secret, &key_wrap_context).unwrap();
    let key_wrap_body = wrapped_file_key.to_json().unwrap().into_bytes();
    let key_envelope = alice_session
        .seal_payload_envelope(
            key_wrap_context.content_context(),
            &SecureMeshPlaintext::new(SecureMeshPayloadKind::Command, key_wrap_body)
                .with_content_type(SECURE_MESH_FILE_KEY_ENVELOPE_CONTENT_TYPE),
        )
        .unwrap();
    let opened_key = bob_session
        .open_payload_envelope(&key_envelope, SecureMeshPayloadKind::Command)
        .unwrap();
    assert_eq!(
        opened_key.content_type.as_deref(),
        Some(SECURE_MESH_FILE_KEY_ENVELOPE_CONTENT_TYPE)
    );
    let wrapped_file_key =
        FileKeyEnvelope::from_json(std::str::from_utf8(&opened_key.body).unwrap()).unwrap();
    let recovered_key = open_file_root_key_for_pairwise_device(
        &wrapped_file_key,
        &wrap_secret,
        &key_wrap_context,
        1_700_000_000,
    )
    .unwrap();

    let opened_manifest =
        open_file_manifest(&recovered_key, &manifest_context, &encrypted_manifest).unwrap();
    let opened_chunk = open_file_chunk(&recovered_key, &chunk_context, &encrypted_chunk).unwrap();
    assert_eq!(opened_manifest, manifest);
    assert_eq!(opened_chunk, chunk);

    let replay_error = bob_session
        .open_payload_envelope(&key_envelope, SecureMeshPayloadKind::Command)
        .unwrap_err();
    assert!(replay_error.to_string().contains("replay"));
}

#[test]
fn secure_mesh_file_key_wraps_out_of_order_and_revocation_fails_closed() {
    let (mut alice_session, mut bob_session) = pairwise_file_sessions();
    let first_key_bytes = [51u8; 32];
    let second_key_bytes = [52u8; 32];
    let first = encrypted_pairwise_file_fixture(
        &alice_session,
        "first",
        first_key_bytes,
        b"first out-of-order file",
    );
    let second = encrypted_pairwise_file_fixture(
        &alice_session,
        "second",
        second_key_bytes,
        b"second out-of-order file",
    );

    let first_envelope = pairwise_file_key_envelope(&mut alice_session, &first);
    let second_envelope = pairwise_file_key_envelope(&mut alice_session, &second);

    let second_opened = bob_session
        .open_payload_envelope(&second_envelope, SecureMeshPayloadKind::Command)
        .unwrap();
    assert_eq!(bob_session.skipped_key_count(), 1);
    let second_recovered = recovered_file_root_key(&second_opened.body, &second);
    assert_eq!(
        open_file_chunk(
            &second_recovered,
            &second.chunk_context,
            &second.encrypted_chunk
        )
        .unwrap()
        .bytes,
        second.chunk.bytes
    );

    let first_opened = bob_session
        .open_payload_envelope(&first_envelope, SecureMeshPayloadKind::Command)
        .unwrap();
    assert_eq!(bob_session.skipped_key_count(), 0);
    let first_recovered = recovered_file_root_key(&first_opened.body, &first);
    assert_eq!(
        open_file_chunk(
            &first_recovered,
            &first.chunk_context,
            &first.encrypted_chunk
        )
        .unwrap()
        .bytes,
        first.chunk.bytes
    );

    let revoked =
        encrypted_pairwise_file_fixture(&alice_session, "revoked", [53u8; 32], b"revoked file");
    let revoked_envelope = pairwise_file_key_envelope(&mut alice_session, &revoked);
    bob_session.revoke();
    let revoked_error = bob_session
        .open_payload_envelope(&revoked_envelope, SecureMeshPayloadKind::Command)
        .unwrap_err();
    assert!(revoked_error.to_string().contains("revoked"));
}
