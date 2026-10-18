use super::support::*;

#[test]
fn secure_mesh_openmls_group_application_message_round_trips() {
    let alice = SecureMeshMlsParticipant::new(b"desktop_gui:alice".to_vec()).unwrap();
    let bob = SecureMeshMlsParticipant::new(b"mobile:bob".to_vec()).unwrap();
    let bob_key_package = bob.generate_key_package().unwrap();
    assert!(!bob_key_package.as_public_bytes().is_empty());

    let mut alice_group = SecureMeshMlsGroup::create(&alice, b"secure-mesh-group-test").unwrap();
    let welcome = alice_group.add_member(&alice, &bob_key_package).unwrap();
    assert!(!welcome.commit_message.is_empty());
    assert!(!welcome.welcome_message.is_empty());

    let mut bob_group =
        SecureMeshMlsGroup::join_from_welcome(&bob, &welcome.welcome_message).unwrap();
    let aad = b"secure-mesh:env_test:msg_test:mailbox_test";
    let sealed = alice_group
        .seal_application_message(&alice, aad, br#"{"op":"secure_mesh.group.commit"}"#)
        .unwrap();
    assert!(!sealed.windows(6).any(|window| window == b"group."));
    let opened = bob_group
        .open_application_message(&bob, aad, &sealed)
        .unwrap();
    assert_eq!(opened, br#"{"op":"secure_mesh.group.commit"}"#);
}

#[test]
fn secure_mesh_openmls_group_application_message_rejects_aad_tamper() {
    let alice = SecureMeshMlsParticipant::new(b"desktop_gui:alice".to_vec()).unwrap();
    let bob = SecureMeshMlsParticipant::new(b"mobile:bob".to_vec()).unwrap();
    let bob_key_package = bob.generate_key_package().unwrap();
    let mut alice_group =
        SecureMeshMlsGroup::create(&alice, b"secure-mesh-group-aad-test").unwrap();
    let welcome = alice_group.add_member(&alice, &bob_key_package).unwrap();
    let mut bob_group =
        SecureMeshMlsGroup::join_from_welcome(&bob, &welcome.welcome_message).unwrap();
    let sealed = alice_group
        .seal_application_message(&alice, b"aad:original", b"body")
        .unwrap();
    let error = bob_group
        .open_application_message(&bob, b"aad:tampered", &sealed)
        .unwrap_err();
    assert!(error.to_string().contains("AAD mismatch"));
}

#[test]
fn secure_mesh_openmls_group_application_message_rejects_replay() {
    let alice = SecureMeshMlsParticipant::new(b"desktop_gui:alice".to_vec()).unwrap();
    let bob = SecureMeshMlsParticipant::new(b"mobile:bob".to_vec()).unwrap();
    let bob_key_package = bob.generate_key_package().unwrap();
    let mut alice_group =
        SecureMeshMlsGroup::create(&alice, b"secure-mesh-group-replay-test").unwrap();
    let welcome = alice_group.add_member(&alice, &bob_key_package).unwrap();
    let mut bob_group =
        SecureMeshMlsGroup::join_from_welcome(&bob, &welcome.welcome_message).unwrap();

    let aad = b"secure-mesh:mls:application-replay";
    let sealed = alice_group
        .seal_application_message(&alice, aad, b"replay-once-only")
        .unwrap();
    assert_eq!(
        bob_group
            .open_application_message(&bob, aad, &sealed)
            .unwrap(),
        b"replay-once-only"
    );
    let replay_error = bob_group
        .open_application_message(&bob, aad, &sealed)
        .unwrap_err();
    assert!(
        replay_error.to_string().contains("open failed")
            || replay_error.to_string().contains("epoch")
            || replay_error.to_string().contains("replay")
    );
}

#[test]
fn secure_mesh_openmls_group_application_message_rejects_stale_epoch() {
    let alice = SecureMeshMlsParticipant::new(b"desktop_gui:alice".to_vec()).unwrap();
    let bob = SecureMeshMlsParticipant::new(b"mobile:bob".to_vec()).unwrap();
    let bob_key_package = bob.generate_key_package().unwrap();
    let mut alice_group =
        SecureMeshMlsGroup::create(&alice, b"secure-mesh-group-stale-epoch-test").unwrap();
    let welcome = alice_group.add_member(&alice, &bob_key_package).unwrap();
    let mut bob_group =
        SecureMeshMlsGroup::join_from_welcome(&bob, &welcome.welcome_message).unwrap();

    let stale_aad = b"secure-mesh:mls:stale-epoch";
    let stale_message = alice_group
        .seal_application_message(&alice, stale_aad, b"old-epoch-message")
        .unwrap();
    let update_commit = alice_group.self_update(&alice).unwrap();
    bob_group.process_commit(&bob, &update_commit).unwrap();
    let stale_error = bob_group
        .open_application_message(&bob, stale_aad, &stale_message)
        .unwrap_err();
    assert!(
        stale_error.to_string().contains("open failed")
            || stale_error.to_string().contains("epoch")
    );
}

#[test]
fn secure_mesh_openmls_concurrent_commits_reject_losing_epoch() {
    let alice = SecureMeshMlsParticipant::new(b"desktop_gui:alice".to_vec()).unwrap();
    let bob = SecureMeshMlsParticipant::new(b"mobile:bob".to_vec()).unwrap();
    let bob_key_package = bob.generate_key_package().unwrap();
    let mut alice_group =
        SecureMeshMlsGroup::create(&alice, b"secure-mesh-group-concurrent-commit-test").unwrap();
    let welcome = alice_group.add_member(&alice, &bob_key_package).unwrap();
    let mut bob_group =
        SecureMeshMlsGroup::join_from_welcome(&bob, &welcome.welcome_message).unwrap();

    let alice_concurrent_commit = alice_group.self_update(&alice).unwrap();
    let bob_concurrent_commit = bob_group.self_update(&bob).unwrap();
    let bob_error = bob_group
        .process_commit(&bob, &alice_concurrent_commit)
        .unwrap_err();
    let alice_error = alice_group
        .process_commit(&alice, &bob_concurrent_commit)
        .unwrap_err();
    assert!(
        bob_error.to_string().contains("commit process failed")
            || bob_error.to_string().contains("epoch")
    );
    assert!(
        alice_error.to_string().contains("commit process failed")
            || alice_error.to_string().contains("epoch")
    );
}
