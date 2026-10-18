use super::support::*;

#[test]
fn client_update_rejects_absolute_and_parent_staged_file_names() {
    let fixture = UpdateFixture::new();
    let absolute = ["/", "test-data", "/", "update.bin"].concat();
    for invalid in [absolute.as_str(), "../update.bin", "nested/update.bin"] {
        let mut artifact = fixture.artifact(TARGET_ID);
        artifact["fileName"] = json!(invalid);
        let manifest = fixture
            .sign_manifest(fixture.unsigned_manifest(json!([release("999.0.0", artifact,)])));
        assert!(
            check(&fixture.params(manifest))
                .unwrap_err()
                .to_string()
                .contains("single relative file name")
        );
    }
}

#[cfg(unix)]
#[test]
fn client_update_rejects_symbolic_link_source_and_staging_paths() {
    use std::os::unix::fs::symlink;

    let fixture = UpdateFixture::new();
    let source_link = fixture.root.join("source-link.bin");
    symlink(&fixture.source, &source_link).unwrap();
    let manifest = fixture.manifest();
    let mut source_params = fixture.checked_params(manifest.clone());
    source_params["sourcePath"] = json!(source_link);
    assert!(
        download(&source_params)
            .unwrap_err()
            .to_string()
            .contains("regular file")
    );

    fs::create_dir_all(&fixture.staging).unwrap();
    let outside = fixture.root.join("outside.bin");
    fs::write(&outside, b"outside").unwrap();
    symlink(&outside, fixture.staging.join("artifact.bin")).unwrap();
    assert!(
        download(&fixture.checked_params(manifest))
            .unwrap_err()
            .to_string()
            .contains("regular file")
    );
}

#[test]
fn client_update_rejects_source_path_that_differs_from_signed_file_url() {
    let fixture = UpdateFixture::new();
    let alternate = fixture.root.join("alternate.bin");
    fs::copy(&fixture.source, &alternate).unwrap();
    let mut params = fixture.checked_params(fixture.manifest());
    params["sourcePath"] = json!(alternate);
    assert!(
        download(&params)
            .unwrap_err()
            .to_string()
            .contains("does not match the signed artifact url")
    );
}
