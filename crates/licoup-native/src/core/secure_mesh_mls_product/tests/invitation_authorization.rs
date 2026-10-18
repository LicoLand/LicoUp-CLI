use super::support::*;

#[test]
fn secure_mesh_mls_product_commit_sender_roster_and_epoch_lag() {
    let roster = BTreeSet::from(["desktop_gui:alice".to_string(), "mobile:bob".to_string()]);
    authorize_commit_sender("desktop_gui:alice", &DeviceTrustState::Verified, &roster).unwrap();
    let outsider =
        authorize_commit_sender("mobile:eve", &DeviceTrustState::Verified, &roster).unwrap_err();
    assert!(outsider.to_string().contains("not in the verified roster"));
    authorize_epoch_lag(5, 5).unwrap();
    authorize_epoch_lag(5, 3).unwrap();
    let stale = authorize_epoch_lag(5, 2).unwrap_err();
    assert!(stale.to_string().contains("epoch lag"));
}

#[test]
fn secure_mesh_mls_product_forged_sender_and_typed_kt_member_add() {
    authorize_sender_endpoint_binding("desktop_gui:alice", "desktop_gui:alice").unwrap();
    let forged =
        authorize_sender_endpoint_binding("mobile:attacker", "desktop_gui:alice").unwrap_err();
    assert!(forged.to_string().contains("forged sender"));

    let bob = device("mobile:bob-kt");
    let key_package = bob.participant.generate_key_package().unwrap();
    let authorization = authorized_member_add_directory(
        &bob,
        &key_package,
        7,
        11,
        capability_now(),
        DirectoryAuthorizationPurpose::MlsMemberAdd,
    );
    authorize_member_add_with_directory(&authorization, &bob.identity, &key_package, 7, 11)
        .unwrap();

    let wrong_identity = device("mobile:eve-kt");
    let identity_error = authorize_member_add_with_directory(
        &authorization,
        &wrong_identity.identity,
        &key_package,
        7,
        11,
    )
    .unwrap_err();
    assert!(identity_error.to_string().contains("identity commitment"));

    let directory_version_error =
        authorize_member_add_with_directory(&authorization, &bob.identity, &key_package, 8, 11)
            .unwrap_err();
    assert!(
        directory_version_error
            .to_string()
            .contains("publication version")
    );

    let key_package_version_error =
        authorize_member_add_with_directory(&authorization, &bob.identity, &key_package, 7, 12)
            .unwrap_err();
    assert!(
        key_package_version_error
            .to_string()
            .contains("KeyPackage commitment")
    );

    let substituted_key_package = bob.participant.generate_key_package().unwrap();
    let key_package_digest_error = authorize_member_add_with_directory(
        &authorization,
        &bob.identity,
        &substituted_key_package,
        7,
        11,
    )
    .unwrap_err();
    assert!(
        key_package_digest_error
            .to_string()
            .contains("KeyPackage commitment")
    );

    let wrong_purpose = authorized_member_add_directory(
        &bob,
        &key_package,
        7,
        11,
        capability_now(),
        DirectoryAuthorizationPurpose::Pairing,
    );
    let purpose_error =
        authorize_member_add_with_directory(&wrong_purpose, &bob.identity, &key_package, 7, 11)
            .unwrap_err();
    assert!(purpose_error.to_string().contains("purpose mismatch"));
}
