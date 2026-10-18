//! Unit tests for the declared-identity rules.
//!
//! The store's own behaviour is exercised next to the crate in
//! `tests/project_identity_registration.rs`; these cases are the parsers, which
//! decide what an identity may be before any durable owner sees it.

use super::*;

#[test]
fn a_declared_identifier_refuses_a_location_or_an_empty_value() {
    assert!(ProjectId::declare("lico-up").is_ok());
    assert!(ProjectId::declare("project:alpha").is_ok());
    for refused in [
        "",
        "   ",
        "../etc",
        "a/b",
        "a\\b",
        "C:\\src",
        ".hidden-ok-but-empty-first",
        "with space",
        "with\0nul",
    ] {
        let failure = ProjectId::declare(refused).expect_err("must be refused");
        assert_eq!(failure.code(), "project_identity_required");
        assert_eq!(failure.stage(), IDENTITY_STAGE);
    }
    assert!(ProjectId::declare("x".repeat(MAX_PROJECT_ID_BYTES + 1)).is_err());
    assert!(WorkspaceId::declare("").is_err());
    assert!(PlanId::declare("").is_err());
}

#[test]
fn a_declared_root_must_be_an_absolute_location() {
    assert!(AuthorizedRoot::declare("/synthetic/authorized/root").is_ok());
    for refused in ["", "  ", "relative/root", "./root", "\0"] {
        let failure = AuthorizedRoot::declare(refused).expect_err("must be refused");
        assert_eq!(failure.code(), "project_authorized_root_required");
    }
    assert!(AuthorizedRoot::declare("x".repeat(MAX_AUTHORIZED_ROOT_BYTES + 1)).is_err());
}

#[test]
fn an_authority_reference_is_bounded_and_typed() {
    let reference = AuthorityReference::membership("membership:owner").expect("a membership");
    assert_eq!(reference.kind(), AuthorityKind::Membership);
    assert_eq!(reference.reference(), "membership:owner");
    assert_eq!(reference.to_string(), "membership membership:owner");
    assert_eq!(AuthorityKind::parse("grant"), Some(AuthorityKind::Grant));
    assert_eq!(AuthorityKind::parse("owner"), None);
    for refused in ["", "   ", "\0"] {
        assert!(
            AuthorityReference::declare(AuthorityKind::Role, refused).is_err(),
            "{refused} must be refused"
        );
    }
    assert!(
        AuthorityReference::declare(
            AuthorityKind::Role,
            "r".repeat(MAX_AUTHORITY_REFERENCE_BYTES + 1)
        )
        .is_err()
    );
}

/// A deserialized identity is validated on the same path as a constructed one,
/// so a wire payload cannot smuggle an unusable identity past the parser.
#[test]
fn deserializing_an_identity_applies_the_declaration_rule() {
    let decoded: Result<AuthorityReference, _> = serde_json::from_value(serde_json::json!({
        "kind": "membership",
        "reference": "membership:owner",
    }));
    assert!(decoded.is_ok());
    let refused: Result<AuthorityReference, _> = serde_json::from_value(serde_json::json!({
        "kind": "membership",
        "reference": "",
    }));
    assert!(refused.is_err());
    let unknown_kind: Result<AuthorityReference, _> = serde_json::from_value(serde_json::json!({
        "kind": "owner",
        "reference": "membership:owner",
    }));
    assert!(unknown_kind.is_err());
    let refused_id: Result<ProjectId, _> = serde_json::from_value(serde_json::json!("../etc"));
    assert!(refused_id.is_err());
}

#[test]
fn a_declared_work_item_identity_refuses_a_location_or_an_empty_value() {
    assert!(WorkItemId::declare("build").is_ok());
    assert!(WorkItemId::declare("item:package-1").is_ok());
    for refused in ["", "   ", "../etc", "a/b", "with space", "with\0nul"] {
        let failure = WorkItemId::declare(refused).expect_err("must be refused");
        assert_eq!(failure.code(), "project_work_item_identity_required");
        assert_eq!(failure.stage(), IDENTITY_STAGE);
    }
    assert!(WorkItemId::declare("x".repeat(MAX_WORK_ITEM_ID_BYTES + 1)).is_err());
}

#[test]
fn a_declared_artifact_reference_keeps_its_two_shapes() {
    let item = WorkItemId::declare("build").expect("a bounded identity");
    let local = ArtifactReference::local(item.clone(), "dist/out.bin").expect("a bounded location");
    assert_eq!(local.kind(), "local");
    assert_eq!(local.local_path(), Some("dist/out.bin"));
    assert_eq!(
        local.producer(&ProjectId::declare("alpha").expect("a bounded identity")),
        WorkRef::new(
            ProjectId::declare("alpha").expect("a bounded identity"),
            item.clone()
        )
    );
    assert_eq!(local.to_string(), "local dist/out.bin");

    let cross = ArtifactReference::cross_project(
        ProjectId::declare("bravo").expect("a bounded identity"),
        item,
    );
    assert_eq!(cross.kind(), "cross-project");
    assert_eq!(cross.local_path(), None);
    assert_eq!(
        cross.producer(&ProjectId::declare("alpha").expect("a bounded identity")),
        WorkRef::new(
            ProjectId::declare("bravo").expect("a bounded identity"),
            WorkItemId::declare("build").expect("a bounded identity")
        )
    );
    assert_eq!(cross.to_string(), "cross-project bravo/build");
}

#[test]
fn a_declared_location_is_bounded_and_must_stay_inside_its_authorized_root() {
    let root = AuthorizedRoot::declare("/synthetic/authorized/alpha").expect("an absolute root");
    for accepted in ["out.bin", "build/out.bin", "./build/out.bin"] {
        assert!(
            stays_inside_authorized_root(&root, accepted),
            "{accepted} stays inside the root"
        );
    }
    for refused in [
        "",
        "   ",
        "..",
        "../alpha-secret",
        "build/../../outside",
        "build/..",
        "/etc/passwd",
        "with\0nul",
    ] {
        assert!(
            !stays_inside_authorized_root(&root, refused),
            "{refused} must be refused"
        );
    }
    let too_long = "x".repeat(MAX_ARTIFACT_PATH_BYTES + 1);
    assert!(!stays_inside_authorized_root(&root, &too_long));

    let item = WorkItemId::declare("build").expect("a bounded identity");
    for refused in ["", "   ", &"x".repeat(MAX_ARTIFACT_PATH_BYTES + 1)] {
        let failure = ArtifactReference::local(item.clone(), refused).expect_err("must be refused");
        assert_eq!(failure.code(), "project_artifact_path_required");
        assert_eq!(failure.stage(), IDENTITY_STAGE);
    }
}

/// A dependency payload is validated on the same path as a constructed one, and
/// a field this owner did not declare is refused rather than dropped.
#[test]
fn deserializing_a_dependency_applies_the_declaration_rule() {
    let decoded: Result<WorkDependency, _> = serde_json::from_value(serde_json::json!({
        "projectId": "alpha",
        "workItemId": "test",
        "artifact": {
            "kind": "local",
            "producerWorkItemId": "build",
            "path": "dist/out.bin",
        },
    }));
    assert_eq!(
        decoded.expect("a declared local dependency decodes"),
        WorkDependency {
            project_id: ProjectId::declare("alpha").expect("a bounded identity"),
            work_item_id: WorkItemId::declare("test").expect("a bounded identity"),
            artifact: ArtifactReference::local(
                WorkItemId::declare("build").expect("a bounded identity"),
                "dist/out.bin",
            )
            .expect("a bounded location"),
        }
    );

    let cross: Result<WorkDependency, _> = serde_json::from_value(serde_json::json!({
        "projectId": "alpha",
        "workItemId": "test",
        "artifact": {"kind": "cross-project", "projectId": "bravo", "workItemId": "build"},
    }));
    assert_eq!(
        cross
            .expect("a declared cross-project dependency decodes")
            .producer(),
        WorkRef::new(
            ProjectId::declare("bravo").expect("a bounded identity"),
            WorkItemId::declare("build").expect("a bounded identity")
        )
    );

    let empty_path: Result<WorkDependency, _> = serde_json::from_value(serde_json::json!({
        "projectId": "alpha",
        "workItemId": "test",
        "artifact": {"kind": "local", "producerWorkItemId": "build", "path": ""},
    }));
    assert!(empty_path.is_err());

    let smuggled: Result<WorkDependency, _> = serde_json::from_value(serde_json::json!({
        "projectId": "alpha",
        "workItemId": "test",
        "artifact": {
            "kind": "local",
            "producerWorkItemId": "build",
            "path": "dist/out.bin",
            "credential": "synthetic-secret-material",
        },
    }));
    assert!(
        smuggled.is_err(),
        "an undeclared field is refused rather than dropped"
    );

    let refused_location: Result<WorkDependency, _> = serde_json::from_value(serde_json::json!({
        "projectId": "alpha",
        "workItemId": "test",
        "artifact": {"kind": "local", "producerWorkItemId": "../etc", "path": "dist/out.bin"},
    }));
    assert!(refused_location.is_err());
}

/// The read side never joins a location that would replace the declared root,
/// so even a stored row that named one stays inside the declaration.
#[test]
fn reading_a_declared_location_never_leaves_its_authorized_root() {
    let root = AuthorizedRoot::declare("/synthetic/authorized/alpha").expect("an absolute root");
    for unusable in [
        "/etc/passwd",
        "../outside/secret.txt",
        "dist/../../outside.txt",
    ] {
        assert_eq!(
            read_local_artifact(&root, unusable),
            ArtifactState::Unavailable,
            "{unusable} must not be joined onto the root"
        );
    }
    assert_eq!(read_local_artifact(&root, ""), ArtifactState::Missing);
    assert_eq!(
        read_local_artifact(&root, "dist/out.bin"),
        ArtifactState::Missing,
        "a location under an absent root is explicitly missing"
    );
}

#[test]
fn reading_a_declared_location_reports_materialization_from_the_declared_root() {
    let base = std::env::temp_dir().join(format!(
        "licoup-project-artifact-read-{}",
        std::process::id()
    ));
    let directory = base.join("alpha");
    std::fs::create_dir_all(directory.join("dist")).expect("the synthetic root is creatable");
    std::fs::write(directory.join("dist/out.bin"), b"synthetic").expect("the artifact is writable");
    let root = AuthorizedRoot::declare(directory.to_string_lossy().into_owned())
        .expect("an absolute root");

    assert_eq!(
        read_local_artifact(&root, "dist/out.bin"),
        ArtifactState::Materialized
    );
    assert_eq!(
        read_local_artifact(&root, "dist/absent.bin"),
        ArtifactState::Missing
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// A declared change is a wire contract too: a payload this owner did not
/// declare is refused rather than dropped, and both shapes of input decode.
#[test]
fn a_declared_change_decodes_only_its_own_fields() {
    let decoded: ChangeRequest = serde_json::from_value(serde_json::json!({
        "projectId": "alpha",
        "changes": [{
            "workItemId": "test",
            "inputs": [
                {"kind": "local", "producerWorkItemId": "build", "path": "dist/out.bin"},
                {"kind": "cross-project", "projectId": "bravo", "workItemId": "build"},
            ],
        }],
    }))
    .expect("a declared change decodes");
    assert_eq!(
        decoded.project_id,
        ProjectId::declare("alpha").expect("a bounded identity")
    );
    assert_eq!(decoded.changes.len(), 1);
    assert_eq!(decoded.changes[0].inputs.len(), 2);
    assert_eq!(
        decoded.changes[0].inputs[1],
        ArtifactReference::cross_project(
            ProjectId::declare("bravo").expect("a bounded identity"),
            WorkItemId::declare("build").expect("a bounded identity"),
        )
    );

    let smuggled: Result<ChangeRequest, _> = serde_json::from_value(serde_json::json!({
        "projectId": "alpha",
        "changes": [],
        "authority": {"kind": "grant", "reference": "grant:owner"},
    }));
    assert!(
        smuggled.is_err(),
        "an undeclared field is refused rather than dropped"
    );

    let refused_input: Result<ChangeRequest, _> = serde_json::from_value(serde_json::json!({
        "projectId": "alpha",
        "changes": [{
            "workItemId": "test",
            "inputs": [
                {"kind": "local", "producerWorkItemId": "../etc", "path": "dist/out.bin"},
            ],
        }],
    }));
    assert!(refused_input.is_err());

    // The answer carries the same declared values, one stable spelling each.
    let preview = ChangePreview {
        project_id: ProjectId::declare("alpha").expect("a bounded identity"),
        affected: vec![AffectedWorkItem {
            work: WorkRef::new(
                ProjectId::declare("alpha").expect("a bounded identity"),
                WorkItemId::declare("test").expect("a bounded identity"),
            ),
            path: vec![WorkRef::new(
                ProjectId::declare("alpha").expect("a bounded identity"),
                WorkItemId::declare("test").expect("a bounded identity"),
            )],
            impact: ChangeImpact::Consumer,
            inputs: vec![DeclaredInputState {
                producer: WorkRef::new(
                    ProjectId::declare("alpha").expect("a bounded identity"),
                    WorkItemId::declare("build").expect("a bounded identity"),
                ),
                artifact: ArtifactReference::local(
                    WorkItemId::declare("build").expect("a bounded identity"),
                    "dist/absent.bin",
                )
                .expect("a declared location"),
                state: ArtifactState::Missing,
                pending: true,
            }],
            activity: WorkActivity::Accepted,
            handoff: ChangeHandoff::Required,
        }],
        untouched_projects: vec![ProjectId::declare("bravo").expect("a bounded identity")],
    };
    let encoded = serde_json::to_value(&preview).expect("the answer encodes");
    assert_eq!(encoded["affected"][0]["impact"], "consumer");
    assert_eq!(encoded["affected"][0]["handoff"], "required");
    assert_eq!(encoded["affected"][0]["activity"], "accepted");
    assert_eq!(encoded["affected"][0]["inputs"][0]["state"], "missing");
    assert_eq!(encoded["affected"][0]["inputs"][0]["pending"], true);
    assert_eq!(encoded["untouchedProjects"][0], "bravo");
}

#[test]
fn an_activity_answer_round_trips_through_its_own_spelling() {
    for activity in [
        WorkActivity::NotStarted,
        WorkActivity::InFlight,
        WorkActivity::Accepted,
        WorkActivity::Unknown,
    ] {
        assert_eq!(WorkActivity::parse(activity.as_str()), Some(activity));
        let encoded = serde_json::to_value(activity).expect("the answer encodes");
        assert_eq!(
            encoded,
            serde_json::Value::String(activity.as_str().to_owned())
        );
        let decoded: WorkActivity = serde_json::from_value(encoded).expect("the answer decodes");
        assert_eq!(decoded, activity);
    }
    assert_eq!(WorkActivity::parse("running"), None);
}
