use super::super::params::{allowed_transition, product_version, target_release_track};
use super::support::*;
use crate::domain::client_state_migration::ReleaseTrack;

#[test]
fn client_update_release_track_transition_matrix_is_closed() {
    assert!(allowed_transition(
        ReleaseTrack::Nightly,
        ReleaseTrack::Nightly
    ));
    assert!(allowed_transition(
        ReleaseTrack::Nightly,
        ReleaseTrack::Stable
    ));
    assert!(allowed_transition(
        ReleaseTrack::Stable,
        ReleaseTrack::Stable
    ));
    assert!(!allowed_transition(
        ReleaseTrack::Stable,
        ReleaseTrack::Nightly
    ));
}

#[test]
fn client_update_running_identity_is_not_caller_spoofable() {
    assert!(product_version(&json!({"currentVersion": "999.0.0"})).is_err());
    assert!(target_release_track(&json!({"channel": "stable"})).is_err());
}

#[test]
fn client_update_selects_the_highest_valid_semver_for_track_and_target() {
    let fixture = UpdateFixture::new();
    let manifest = fixture.sign_manifest(fixture.unsigned_manifest(json!([
        release("999.0.2", fixture.artifact(TARGET_ID)),
        release("999.0.1", fixture.artifact(TARGET_ID)),
        release("1000.0.0", fixture.artifact("other-target")),
    ])));
    let result = check(&fixture.params(manifest)).unwrap();
    assert_eq!(result["availableVersion"], "999.0.2");
    assert_eq!(result["artifact"]["targetId"], TARGET_ID);
}

#[test]
fn client_update_ignores_build_metadata_when_comparing_versions() {
    let fixture = UpdateFixture::new();
    let manifest = fixture.unsigned_manifest(json!([release(
        "1.0.0+newer-build",
        fixture.artifact(TARGET_ID)
    ),]));
    let selected = super::super::release::select_highest_release(
        &manifest,
        "1.0.0+earlier-build",
        TARGET_ID,
        true,
    )
    .unwrap();
    assert!(selected.is_none());

    let mut next = release("1.1.0", fixture.artifact(TARGET_ID));
    next["minimumSupportedVersion"] = json!("1.0.0+newer-build");
    let manifest = fixture.unsigned_manifest(json!([next]));
    assert!(
        super::super::release::select_highest_release(
            &manifest,
            "1.0.0+earlier-build",
            TARGET_ID,
            true,
        )
        .unwrap()
        .is_some()
    );
}

#[test]
fn client_update_rejects_prerelease_versions_in_a_stable_manifest() {
    let fixture = UpdateFixture::new();
    let manifest = fixture.sign_manifest(fixture.unsigned_manifest(json!([release(
        "999.0.0-rc.1",
        fixture.artifact(TARGET_ID),
    )])));
    assert!(
        check(&fixture.params(manifest))
            .unwrap_err()
            .to_string()
            .contains("stable manifest contains a prerelease")
    );
}

#[test]
fn client_update_rejects_malformed_release_instead_of_reporting_up_to_date() {
    let fixture = UpdateFixture::new();
    let manifest = fixture.sign_manifest(fixture.unsigned_manifest(json!([release(
        "not-a-version",
        fixture.artifact(TARGET_ID)
    ),])));
    assert!(
        check(&fixture.params(manifest))
            .unwrap_err()
            .to_string()
            .contains("semantic versioning")
    );
}

#[test]
fn client_update_rejects_manifest_track_mismatch() {
    let fixture = UpdateFixture::new();
    let mut manifest =
        fixture.unsigned_manifest(json!([release("999.0.0", fixture.artifact(TARGET_ID),)]));
    manifest["releaseTrack"] = json!("nightly");
    let manifest = fixture.sign_manifest(manifest);
    assert!(
        check(&fixture.params(manifest))
            .unwrap_err()
            .to_string()
            .contains("release track does not match")
    );
}

#[test]
fn client_update_rejects_unknown_signed_manifest_and_frontier_fields() {
    let fixture = UpdateFixture::new();
    let mut manifest =
        fixture.unsigned_manifest(json!([release("999.0.0", fixture.artifact(TARGET_ID),)]));
    manifest["eligibilityOverride"] = json!(true);
    assert!(
        check(&fixture.params(fixture.sign_manifest(manifest)))
            .unwrap_err()
            .to_string()
            .contains("contract is not closed")
    );

    let mut release = release("999.0.0", fixture.artifact(TARGET_ID));
    release["migrationFrontier"]["runtimeHandler"] = json!("caller-supplied");
    let manifest = fixture.sign_manifest(fixture.unsigned_manifest(json!([release])));
    assert!(
        check(&fixture.params(manifest))
            .unwrap_err()
            .to_string()
            .contains("contract is not closed")
    );
}

#[test]
fn client_update_accepts_a_strictly_extended_candidate_frontier() {
    let fixture = UpdateFixture::new();
    let mut next = release("999.0.0", fixture.artifact(TARGET_ID));
    next["migrationFrontier"]["frontierId"] = json!("licoup-state-next");
    let domains = next["migrationFrontier"]["domains"].as_array_mut().unwrap();
    domains[0]["targetSchemaVersion"] = json!(2);
    domains[0]["requiredStepIds"]
        .as_array_mut()
        .unwrap()
        .push(json!("adaptive-flywheel.1-to-2"));
    let manifest = fixture.sign_manifest(fixture.unsigned_manifest(json!([next])));

    let result = check(&fixture.params(manifest)).unwrap();
    assert_eq!(result["availableVersion"], "999.0.0");
    assert_eq!(
        result["migrationFrontier"]["frontierId"],
        "licoup-state-next"
    );
}

#[test]
fn client_update_rejects_frontier_regression_or_history_rewrite() {
    let fixture = UpdateFixture::new();
    for replacement in [json!([]), json!(["replacement-step"])] {
        let mut next = release("999.0.0", fixture.artifact(TARGET_ID));
        next["migrationFrontier"]["domains"][0]["requiredStepIds"] = replacement;
        let manifest = fixture.sign_manifest(fixture.unsigned_manifest(json!([next])));
        assert!(
            check(&fixture.params(manifest))
                .unwrap_err()
                .to_string()
                .contains("migration frontier")
        );
    }
}
