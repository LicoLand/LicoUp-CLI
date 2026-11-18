use super::support::*;

#[test]
fn client_update_requires_signed_revocation_metadata_when_track_policy_demands_it() {
    let fixture = UpdateFixture::new();
    let mut manifest =
        fixture.unsigned_manifest(json!([release("999.0.0", fixture.artifact(TARGET_ID),)]));
    manifest["releaseTrackPolicy"]["revocationPolicy"] = json!("signed-revocation-list-required");
    let params = fixture.params(fixture.sign_manifest(manifest));
    assert!(
        check(&params)
            .unwrap_err()
            .to_string()
            .contains("revocation list is required")
    );
}

#[test]
fn client_update_rejects_unsigned_revocation_metadata() {
    let fixture = UpdateFixture::new();
    let mut params = fixture.params(fixture.manifest());
    params["revocationList"] = revocation_body();
    assert!(
        check(&params)
            .unwrap_err()
            .to_string()
            .contains("contract is not closed")
    );
}

#[test]
fn client_update_rejects_unknown_signed_revocation_fields() {
    let fixture = UpdateFixture::new();
    let mut body = revocation_body();
    body["allowVersion"] = json!("999.0.0");
    let mut params = fixture.params(fixture.manifest());
    params["revocationList"] = fixture.signed_revocation(body);
    assert!(
        check(&params)
            .unwrap_err()
            .to_string()
            .contains("contract is not closed")
    );
}

#[test]
fn client_update_rejects_revoked_role_version_and_artifact() {
    let fixture = UpdateFixture::new();
    let cases = [
        (
            "revokedKeyIds",
            json!([ONLINE_KEY_ID]),
            "signing key is revoked",
        ),
        (
            "revokedVersions",
            json!(["999.0.0"]),
            "release version is revoked",
        ),
        (
            "revokedArtifactDigests",
            json!([fixture.artifact(TARGET_ID)["sha256"].clone()]),
            "release artifact is revoked",
        ),
    ];
    for (field, value, expected) in cases {
        let mut body = revocation_body();
        body[field] = value;
        let mut params = fixture.params(fixture.manifest());
        params["revocationList"] = fixture.signed_revocation(body);
        assert!(check(&params).unwrap_err().to_string().contains(expected));
    }
}
