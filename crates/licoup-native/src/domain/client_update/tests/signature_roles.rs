use super::support::*;

#[test]
fn client_update_requires_distinct_offline_and_online_role_signatures() {
    let fixture = UpdateFixture::new();
    let unsigned =
        fixture.unsigned_manifest(json!([release("999.0.0", fixture.artifact(TARGET_ID),)]));
    let online_only = sign_document(unsigned.clone(), &[(ONLINE_KEY_ID, &fixture.online)]);
    assert!(
        check(&fixture.params(online_only))
            .unwrap_err()
            .to_string()
            .contains("offline root signature")
    );
    let offline_only = sign_document(unsigned, &[(OFFLINE_KEY_ID, &fixture.offline)]);
    assert!(
        check(&fixture.params(offline_only))
            .unwrap_err()
            .to_string()
            .contains("online signing signature")
    );
}

#[test]
fn client_update_rejects_duplicate_signature_key_ids() {
    let fixture = UpdateFixture::new();
    let mut manifest = fixture.manifest();
    let duplicate = manifest["signatures"][0].clone();
    manifest["signatures"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert!(
        check(&fixture.params(manifest))
            .unwrap_err()
            .to_string()
            .contains("must be unique")
    );
}

#[test]
fn client_update_accepts_only_the_formal_keys_wrapper() {
    let fixture = UpdateFixture::new();
    let valid = check(&fixture.params(fixture.manifest())).unwrap();
    assert_eq!(valid["verifiedKeyIds"].as_array().unwrap().len(), 2);

    let mut ambiguous = fixture.params(fixture.manifest());
    ambiguous["publicKeys"] = fixture.public_keys()["keys"].clone();
    assert!(
        check(&ambiguous)
            .unwrap_err()
            .to_string()
            .contains("must contain a keys object")
    );
}
