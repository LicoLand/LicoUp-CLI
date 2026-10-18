use openmls::prelude::{Capabilities, Ciphersuite, CredentialType, ExtensionType};

use crate::core::secure_mesh_mls_pq_epoch::MLS_ML_KEM_1024_EPOCH_EXTENSION_TYPE_ID;

use super::super::constants::MLS_CAPABILITY_EXTENSION_TYPE_ID;
use super::support::*;

/// The leaf capabilities a peer built before the openmls upgrade advertised:
/// the then-default ciphersuite list plus this mesh's extension and credential
/// types. Upgraded peers advertise only the ciphersuite they use.
fn legacy_default_leaf_capabilities() -> Capabilities {
    Capabilities::new(
        None,
        Some(&[
            Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519,
            Ciphersuite::MLS_128_DHKEMP256_AES128GCM_SHA256_P256,
            Ciphersuite::MLS_128_DHKEMX25519_CHACHA20POLY1305_SHA256_Ed25519,
        ]),
        Some(&[
            ExtensionType::Unknown(MLS_CAPABILITY_EXTENSION_TYPE_ID),
            ExtensionType::Unknown(MLS_ML_KEM_1024_EPOCH_EXTENSION_TYPE_ID),
        ]),
        None,
        Some(&[CredentialType::Basic]),
    )
}

/// A peer advertising the pre-upgrade capability set must be able to join an
/// upgraded peer's group, and a group containing it must accept an upgraded
/// member, with application payloads flowing both ways.
#[test]
fn secure_mesh_mls_mixed_capability_peers_join_and_exchange_payloads() {
    let alice = SecureMeshMlsParticipant::new(b"desktop_gui:alice".to_vec()).unwrap();
    let bob = SecureMeshMlsParticipant::new(b"mobile:bob".to_vec()).unwrap();
    let carol = SecureMeshMlsParticipant::new(b"desktop_gui:carol".to_vec()).unwrap();
    let group_id = b"secure-mesh-mixed-capability-group";
    let aad = b"secure-mesh:mls:mixed-capability";

    // Pre-upgrade peer joins the upgraded peer's group.
    let bob_key_package = bob
        .generate_key_package_with_leaf_capabilities(legacy_default_leaf_capabilities())
        .unwrap();
    let mut alice_group = SecureMeshMlsGroup::create(&alice, group_id).unwrap();
    let welcome = alice_group.add_member(&alice, &bob_key_package).unwrap();
    let mut bob_group =
        SecureMeshMlsGroup::join_from_welcome(&bob, &welcome.welcome_message).unwrap();
    assert_eq!(bob_group.epoch(), alice_group.epoch());

    let sealed = alice_group
        .seal_application_message(&alice, aad, b"upgraded-to-legacy")
        .unwrap();
    assert_eq!(
        bob_group
            .open_application_message(&bob, aad, &sealed)
            .unwrap(),
        b"upgraded-to-legacy"
    );
    let sealed = bob_group
        .seal_application_message(&bob, aad, b"legacy-to-upgraded")
        .unwrap();
    assert_eq!(
        alice_group
            .open_application_message(&alice, aad, &sealed)
            .unwrap(),
        b"legacy-to-upgraded"
    );

    // The upgraded peer is joined by the group containing the legacy member;
    // the legacy member must accept the new member's narrower capabilities.
    let carol_key_package = carol.generate_key_package().unwrap();
    let carol_welcome = alice_group.add_member(&alice, &carol_key_package).unwrap();
    bob_group
        .process_commit(&bob, &carol_welcome.commit_message)
        .unwrap();
    let mut carol_group =
        SecureMeshMlsGroup::join_from_welcome(&carol, &carol_welcome.welcome_message).unwrap();
    assert_eq!(carol_group.epoch(), alice_group.epoch());

    let sealed = bob_group
        .seal_application_message(&bob, aad, b"legacy-member-to-joined")
        .unwrap();
    assert_eq!(
        carol_group
            .open_application_message(&carol, aad, &sealed)
            .unwrap(),
        b"legacy-member-to-joined"
    );
    let sealed = carol_group
        .seal_application_message(&carol, aad, b"joined-to-legacy-member")
        .unwrap();
    assert_eq!(
        bob_group
            .open_application_message(&bob, aad, &sealed)
            .unwrap(),
        b"joined-to-legacy-member"
    );
}
