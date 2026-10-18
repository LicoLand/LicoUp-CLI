use super::support::*;

#[test]
fn client_update_verification_uses_only_the_signed_artifact_digest_and_name() {
    let fixture = UpdateFixture::new();
    let manifest = fixture.manifest();
    let params = fixture.checked_params(manifest.clone());
    download(&params).unwrap();
    let verified = verify(&params).unwrap();
    assert_eq!(verified["digestMatched"], true);
    assert_eq!(
        verified["artifactSha256"],
        fixture.artifact(TARGET_ID)["sha256"]
    );

    let mut sha_override = params.clone();
    sha_override["sha256"] = json!(sha256_hex(b"caller-override"));
    assert!(
        verify(&sha_override)
            .unwrap_err()
            .to_string()
            .contains("overrides are forbidden")
    );
    let mut name_override = params;
    name_override["stagedFileName"] = json!("other.bin");
    assert!(
        verify(&name_override)
            .unwrap_err()
            .to_string()
            .contains("overrides are forbidden")
    );
}

#[test]
fn client_update_rejects_tampering_after_download() {
    let fixture = UpdateFixture::new();
    let params = fixture.checked_params(fixture.manifest());
    download(&params).unwrap();
    fs::write(fixture.staging.join("artifact.bin"), b"tampered-update").unwrap();
    assert!(
        verify(&params)
            .unwrap_err()
            .to_string()
            .contains("size does not match signed metadata")
    );
}

#[test]
fn client_update_rejects_signed_file_name_and_url_mismatch() {
    let fixture = UpdateFixture::new();
    let mut artifact = fixture.artifact(TARGET_ID);
    artifact["fileName"] = json!("renamed.bin");
    let manifest =
        fixture.sign_manifest(fixture.unsigned_manifest(json!([release("999.0.0", artifact,)])));
    assert!(
        check(&fixture.params(manifest))
            .unwrap_err()
            .to_string()
            .contains("must match its signed url")
    );
}
