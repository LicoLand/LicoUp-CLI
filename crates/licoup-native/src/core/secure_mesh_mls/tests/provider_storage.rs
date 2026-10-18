use super::support::*;

#[test]
fn secure_mesh_openmls_provider_storage_reload_preserves_group_secrets() {
    let alice = SecureMeshMlsParticipant::new(b"desktop_gui:alice".to_vec()).unwrap();
    let bob = SecureMeshMlsParticipant::new(b"mobile:bob".to_vec()).unwrap();
    let group_id = b"secure-mesh-provider-reload-group";
    let bob_key_package = bob.generate_key_package().unwrap();
    let mut alice_group = SecureMeshMlsGroup::create(&alice, group_id).unwrap();
    let welcome = alice_group.add_member(&alice, &bob_key_package).unwrap();
    let bob_group = SecureMeshMlsGroup::join_from_welcome(&bob, &welcome.welcome_message).unwrap();
    let joined_epoch = bob_group.epoch();
    drop(bob_group);

    let aad = b"secure-mesh:mls:provider-reload";
    let sealed = alice_group
        .seal_application_message(&alice, aad, b"provider-reloaded-open")
        .unwrap();
    let mut reloaded_bob = SecureMeshMlsGroup::load_from_provider(&bob, group_id).unwrap();
    assert_eq!(reloaded_bob.epoch(), joined_epoch);
    assert_eq!(
        reloaded_bob
            .open_application_message(&bob, aad, &sealed)
            .unwrap(),
        b"provider-reloaded-open"
    );

    let update_commit = alice_group.self_update(&alice).unwrap();
    reloaded_bob.process_commit(&bob, &update_commit).unwrap();
    let updated_epoch = reloaded_bob.epoch();
    drop(reloaded_bob);

    let aad_after_update = b"secure-mesh:mls:provider-reload-after-update";
    let sealed_after_update = alice_group
        .seal_application_message(&alice, aad_after_update, b"after-storage-reload-update")
        .unwrap();
    let mut reloaded_after_update = SecureMeshMlsGroup::load_from_provider(&bob, group_id).unwrap();
    assert_eq!(reloaded_after_update.epoch(), updated_epoch);
    assert_eq!(
        reloaded_after_update
            .open_application_message(&bob, aad_after_update, &sealed_after_update)
            .unwrap(),
        b"after-storage-reload-update"
    );
}

#[test]
fn secure_mesh_openmls_secret_store_handle_reload_recovers_group_state() {
    let secret_store = test_secret_store();
    let secret_store_handle =
        test_secret_store_handle("secret-store-reload", MLS_EPOCH_SECRET_STORE_CLASS);
    let alice = SecureMeshMlsParticipant::new(b"desktop_gui:alice".to_vec()).unwrap();
    let bob = SecureMeshMlsParticipant::new(b"mobile:bob".to_vec()).unwrap();
    let bob_signing_public_key = bob.signing_public_key();
    let group_id = b"secure-mesh-secret-store-reload-group";

    let bob_key_package = bob.generate_key_package().unwrap();
    let mut alice_group = SecureMeshMlsGroup::create(&alice, group_id).unwrap();
    let welcome = alice_group.add_member(&alice, &bob_key_package).unwrap();
    let mut bob_group =
        SecureMeshMlsGroup::join_from_welcome(&bob, &welcome.welcome_message).unwrap();

    let update_commit = alice_group.self_update(&alice).unwrap();
    bob_group.process_commit(&bob, &update_commit).unwrap();
    let persisted_epoch = bob_group.epoch();
    bob.save_secret_store(secret_store.as_ref(), &secret_store_handle)
        .unwrap();
    let persisted_secret = secret_store
        .get_secret(&secret_store_handle)
        .unwrap()
        .unwrap();
    let persisted_secret = persisted_secret.expose_utf8().unwrap();
    assert!(persisted_secret.contains(MLS_EPOCH_SECRET_STORE_CLASS));
    assert!(!persisted_secret.contains("secret-store-reloaded-open"));
    drop(bob_group);
    drop(bob);

    let reloaded_bob = SecureMeshMlsParticipant::load_from_secret_store(
        b"mobile:bob".to_vec(),
        &bob_signing_public_key,
        secret_store.as_ref(),
        &secret_store_handle,
    )
    .unwrap();
    let mut reloaded_group =
        SecureMeshMlsGroup::load_from_provider(&reloaded_bob, group_id).unwrap();
    assert_eq!(reloaded_group.epoch(), persisted_epoch);

    let aad = b"secure-mesh:mls:secret-store-file-reload";
    let sealed = alice_group
        .seal_application_message(&alice, aad, b"secret-store-reloaded-open")
        .unwrap();
    assert_eq!(
        reloaded_group
            .open_application_message(&reloaded_bob, aad, &sealed)
            .unwrap(),
        b"secret-store-reloaded-open"
    );
    SecureMeshOpenMlsProvider::delete_secret_store(secret_store.as_ref(), &secret_store_handle)
        .unwrap();
    assert!(
        secret_store
            .get_secret(&secret_store_handle)
            .unwrap()
            .is_none()
    );
}

#[test]
fn secure_mesh_openmls_secret_store_recovery_preserves_authenticated_state() {
    let secret_store = test_secret_store();
    let secret_store_handle =
        test_secret_store_handle("authenticated-recovery", MLS_RECOVERY_SECRET_STORE_CLASS);
    let alice = SecureMeshMlsParticipant::new(b"desktop_gui:alice".to_vec()).unwrap();
    let bob = SecureMeshMlsParticipant::new(b"mobile:bob".to_vec()).unwrap();
    let bob_signing_public_key = bob.signing_public_key();
    let group_id = b"secure-mesh-authenticated-recovery-group";

    let bob_key_package = bob.generate_key_package().unwrap();
    assert!(!bob_key_package.as_public_bytes().is_empty());

    let mut alice_group = SecureMeshMlsGroup::create(&alice, group_id).unwrap();
    let welcome = alice_group.add_member(&alice, &bob_key_package).unwrap();
    deserialize_protocol_message(
        &welcome.commit_message,
        "secure mesh MLS interop vector commit parse failed",
    )
    .unwrap();
    match MlsMessageIn::tls_deserialize_exact(welcome.welcome_message.clone())
        .unwrap()
        .extract()
    {
        MlsMessageBodyIn::Welcome(_) => {}
        _ => panic!("secure mesh MLS interop vector welcome parse failed"),
    }

    let mut bob_group =
        SecureMeshMlsGroup::join_from_welcome(&bob, &welcome.welcome_message).unwrap();
    let capability_commit =
        activate_test_payload_capabilities(&mut alice_group, &alice, &[&alice, &bob]);
    process_test_payload_capability_commit(&mut bob_group, &bob, &capability_commit);
    let update_commit = alice_group.self_update(&alice).unwrap();
    deserialize_protocol_message(
        &update_commit,
        "secure mesh MLS interop vector update parse failed",
    )
    .unwrap();
    bob_group.process_commit(&bob, &update_commit).unwrap();
    let recovered_epoch = bob_group.epoch();
    let secret_store_session = secret_store
        .begin_authorized_session(&SecretStoreAuthorizationRequest::new(
            "Secure Mesh MLS authenticated recovery secret-store authorization batch",
            2,
        ))
        .unwrap();
    bob.save_recovery_secret_store_with_session(
        secret_store.as_ref(),
        &secret_store_handle,
        &secret_store_session,
    )
    .unwrap();
    drop(bob_group);
    drop(bob);

    let reloaded_bob = SecureMeshMlsParticipant::load_from_secret_store_with_optional_session(
        b"mobile:bob".to_vec(),
        &bob_signing_public_key,
        secret_store.as_ref(),
        &secret_store_handle,
        Some(&secret_store_session),
    )
    .unwrap();
    let mut reloaded_bob_group =
        SecureMeshMlsGroup::load_from_provider(&reloaded_bob, group_id).unwrap();
    assert_eq!(reloaded_bob_group.epoch(), recovered_epoch);

    let context = content_context_fixture(
        "msg_mls_authenticated_recovery",
        "desktop_gui:alice",
        "mobile:bob",
        format!("mls:{recovered_epoch}:secure-mesh-authenticated-recovery-group"),
    );
    let body = br#"{"op":"secure_mesh.group.commit","canary":"mls-authenticated-recovery-secret"}"#;
    let plaintext = SecureMeshPlaintext::new(SecureMeshPayloadKind::Command, body.as_slice())
        .with_content_type("application/json");
    let application_message = alice_group
        .seal_payload_message(&alice, &context, &plaintext)
        .unwrap();
    deserialize_protocol_message(
        &application_message,
        "secure mesh MLS interop vector application parse failed",
    )
    .unwrap();
    assert!(
        !application_message
            .windows(b"mls-authenticated-recovery-secret".len())
            .any(|window| window == b"mls-authenticated-recovery-secret")
    );

    let opened = reloaded_bob_group
        .open_payload_message(
            &reloaded_bob,
            &context,
            &application_message,
            SecureMeshPayloadKind::Command,
        )
        .unwrap();
    assert_eq!(opened.body, body);

    let public_artifacts: [(&str, &[u8]); 4] = [
        ("key_package", bob_key_package.as_public_bytes()),
        ("welcome", &welcome.welcome_message),
        ("commit", &update_commit),
        ("application", &application_message),
    ];
    for (label, bytes) in public_artifacts {
        assert!(!bytes.is_empty(), "{label} artifact must be non-empty");
        let hash = hash_bytes(bytes);
        assert!(hash.starts_with("sha256:"));
        assert!(
            !bytes
                .windows(b"mls-authenticated-recovery-secret".len())
                .any(|window| window == b"mls-authenticated-recovery-secret"),
            "{label} artifact leaked plaintext canary"
        );
    }
    SecureMeshOpenMlsProvider::delete_secret_store(secret_store.as_ref(), &secret_store_handle)
        .unwrap();
}
