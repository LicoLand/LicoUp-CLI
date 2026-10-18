//! The project identity family through its production entry.
//!
//! Each case runs the typed command through the port the shared facade calls,
//! so what is tested is the composition the client runs — not a re-implementation
//! of it. The roots are temporary and the authority references are synthetic.

use super::*;
use licoup_application::{
    ActorClaim, ArtifactInputRequest, CommandOutcome, CommandResolution,
    DependencyDeclarationRequest, Operation, OperationState, ProjectCommand, ProjectPort,
    ProjectRegistrationRequest,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// One temporary data root, removed when the case ends.
struct TempRoot {
    path: std::path::PathBuf,
}

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "licoup-native-project-{label}-{}-{nanos}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("the temporary root is creatable");
        Self { path }
    }

    fn declared_root(&self, label: &str) -> String {
        self.path
            .join("authorized")
            .join(label)
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn request(
    project_id: &str,
    workspace_id: &str,
    plan_id: &str,
    authorized_root: &str,
    authority_reference: &str,
) -> ProjectRegistrationRequest {
    ProjectRegistrationRequest {
        project_id: project_id.to_owned(),
        display_name: format!("Synthetic project {project_id}"),
        authorized_root: authorized_root.to_owned(),
        authority_kind: "membership".to_owned(),
        authority_reference: authority_reference.to_owned(),
        workspace_id: workspace_id.to_owned(),
        plan_id: plan_id.to_owned(),
    }
}

fn outcome(resolution: CommandResolution) -> CommandOutcome {
    match resolution {
        CommandResolution::Resolved(outcome) => outcome,
        CommandResolution::Failed(failure) => {
            panic!(
                "expected a resolved command, got {} ({})",
                failure.code, failure.stage
            )
        }
    }
}

fn failed(resolution: CommandResolution) -> licoup_application::ApplicationFailure {
    match resolution {
        CommandResolution::Resolved(outcome) => {
            panic!("expected a refusal, got {outcome:?}")
        }
        CommandResolution::Failed(failure) => failure,
    }
}

fn execute(
    application: &NativeProjectApplication,
    claim: &ActorClaim,
    command: ProjectCommand,
) -> CommandResolution {
    match application.execute(claim, &command) {
        Ok(outcome) => CommandResolution::Resolved(outcome),
        Err(failure) => CommandResolution::Failed(failure),
    }
}

#[test]
fn two_synthetic_authorized_projects_register_and_read_back_through_the_port() {
    let root = TempRoot::new("two-projects");
    let application = NativeProjectApplication::at(&root.path);
    let claim = ActorClaim::local_admin("membership:owner");

    let alpha = outcome(execute(
        &application,
        &claim,
        ProjectCommand::Register(request(
            "alpha-project",
            "workspace:shared",
            "plan:alpha",
            &root.declared_root("alpha"),
            "membership:owner",
        )),
    ));
    assert_eq!(alpha.reference.operation, "project.register");
    assert_eq!(alpha.reference.id, "alpha-project");
    assert_eq!(alpha.reference.state, OperationState::Completed);
    assert_eq!(alpha.payload["projectId"], "alpha-project");
    assert_eq!(alpha.payload["authorityKind"], "membership");
    assert_eq!(alpha.payload["authorityReference"], "membership:owner");
    assert_eq!(alpha.payload["registrationSequence"], 1);

    let bravo = outcome(execute(
        &application,
        &claim,
        ProjectCommand::Register(request(
            "bravo-project",
            "workspace:shared",
            "plan:bravo",
            &root.declared_root("bravo"),
            "membership:owner",
        )),
    ));
    assert_eq!(bravo.payload["registrationSequence"], 2);

    let read = outcome(execute(
        &application,
        &claim,
        ProjectCommand::Read {
            project_id: "alpha-project".to_owned(),
        },
    ));
    assert_eq!(read.reference.operation, "project.read");
    assert_eq!(read.payload["project"]["projectId"], "alpha-project");
    assert_eq!(read.payload["project"]["planId"], "plan:alpha");

    let missing = outcome(execute(
        &application,
        &claim,
        ProjectCommand::Read {
            project_id: "missing-project".to_owned(),
        },
    ));
    assert_eq!(missing.payload["project"], serde_json::Value::Null);

    let listed = outcome(execute(&application, &claim, ProjectCommand::List));
    assert_eq!(listed.reference.operation, "project.list");
    assert_eq!(
        listed.payload["projects"]
            .as_array()
            .expect("a listing carries projects")
            .iter()
            .map(|project| project["projectId"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["alpha-project", "bravo-project"]
    );
}

/// The durable record a one-shot process wrote is the record the next process
/// reads: a fresh composition over the same root returns the same projects.
#[test]
fn registrations_survive_a_fresh_composition_over_the_same_root() {
    let root = TempRoot::new("reopen");
    let claim = ActorClaim::local_admin("membership:owner");
    {
        let application = NativeProjectApplication::at(&root.path);
        outcome(execute(
            &application,
            &claim,
            ProjectCommand::Register(request(
                "alpha-project",
                "workspace:shared",
                "plan:alpha",
                &root.declared_root("alpha"),
                "membership:owner",
            )),
        ));
    }

    let recompensed = NativeProjectApplication::at(&root.path);
    let listed = outcome(execute(&recompensed, &claim, ProjectCommand::List));
    assert_eq!(
        listed.payload["projects"][0]["projectId"], "alpha-project",
        "the registration must survive a fresh composition"
    );
    // The next registration continues the durable order instead of restarting it.
    let bravo = outcome(execute(
        &recompensed,
        &claim,
        ProjectCommand::Register(request(
            "bravo-project",
            "workspace:shared",
            "plan:bravo",
            &root.declared_root("bravo"),
            "membership:owner",
        )),
    ));
    assert_eq!(bravo.payload["registrationSequence"], 2);
}

#[test]
fn a_duplicate_identity_is_refused_through_the_port() {
    let root = TempRoot::new("duplicate");
    let application = NativeProjectApplication::at(&root.path);
    let claim = ActorClaim::local_admin("membership:owner");
    outcome(execute(
        &application,
        &claim,
        ProjectCommand::Register(request(
            "alpha-project",
            "workspace:shared",
            "plan:alpha",
            &root.declared_root("alpha"),
            "membership:owner",
        )),
    ));

    let refusal = failed(execute(
        &application,
        &claim,
        ProjectCommand::Register(request(
            "alpha-project",
            "workspace:shared",
            "plan:other",
            &root.declared_root("elsewhere"),
            "membership:owner",
        )),
    ));
    assert_eq!(refusal.code, "project_identity_duplicate");
    assert_eq!(refusal.stage, "project/register");
    assert!(!refusal.retryable, "a duplicate identity is not retryable");
    let listed = outcome(execute(&application, &claim, ProjectCommand::List));
    assert_eq!(listed.payload["projects"].as_array().map(Vec::len), Some(1));
}

#[test]
fn a_caller_cannot_register_a_project_under_another_authority() {
    let root = TempRoot::new("foreign-authority");
    let application = NativeProjectApplication::at(&root.path);
    let claim = ActorClaim::membership("codex", "conversation:one", "membership:codex");

    let refusal = failed(execute(
        &application,
        &claim,
        ProjectCommand::Register(request(
            "alpha-project",
            "workspace:shared",
            "plan:alpha",
            &root.declared_root("alpha"),
            "membership:owner",
        )),
    ));
    assert_eq!(refusal.code, "project_authority_unauthorized");
    assert_eq!(refusal.stage, "project/register");

    // The caller's own membership is the one authority it may register under.
    outcome(execute(
        &application,
        &claim,
        ProjectCommand::Register(request(
            "alpha-project",
            "workspace:shared",
            "plan:alpha",
            &root.declared_root("alpha"),
            "membership:codex",
        )),
    ));
}

#[test]
fn an_unknown_authority_kind_and_an_empty_identity_are_refused() {
    let root = TempRoot::new("invalid");
    let application = NativeProjectApplication::at(&root.path);
    let claim = ActorClaim::local_admin("membership:owner");

    let mut unknown_kind = request(
        "alpha-project",
        "workspace:shared",
        "plan:alpha",
        &root.declared_root("alpha"),
        "membership:owner",
    );
    unknown_kind.authority_kind = "owner".to_owned();
    let refusal = failed(execute(
        &application,
        &claim,
        ProjectCommand::Register(unknown_kind),
    ));
    assert_eq!(refusal.code, "project_authority_reference_required");

    let refusal = failed(execute(
        &application,
        &claim,
        ProjectCommand::Register(request(
            "",
            "workspace:shared",
            "plan:alpha",
            &root.declared_root("alpha"),
            "membership:owner",
        )),
    ));
    assert_eq!(refusal.code, "project_identity_required");

    let listed = outcome(execute(&application, &claim, ProjectCommand::List));
    assert_eq!(listed.payload["projects"], serde_json::json!([]));
}

/// The design decision this node exists for: an identity is declared, never
/// resolved by walking a directory.
#[test]
fn no_registration_reads_the_authorized_root_it_declares() {
    let root = TempRoot::new("no-scan");
    let application = NativeProjectApplication::at(&root.path);
    let claim = ActorClaim::local_admin("membership:owner");
    let declared = root.path.join("authorized").join("alpha");
    std::fs::create_dir_all(&declared).expect("the synthetic root is creatable");
    let marker = declared.join("Cargo.toml");
    std::fs::write(&marker, "[package]\nname = \"synthetic\"\n").expect("the marker is writable");

    outcome(execute(
        &application,
        &claim,
        ProjectCommand::Register(request(
            "alpha-project",
            "workspace:shared",
            "plan:alpha",
            &declared.to_string_lossy(),
            "membership:owner",
        )),
    ));
    assert_eq!(
        std::fs::read_to_string(&marker).expect("the marker is readable"),
        "[package]\nname = \"synthetic\"\n",
        "registration must not have read or rewritten the declared root"
    );
    let store = application.store().expect("the store is composable");
    assert!(
        store
            .database_path()
            .starts_with(root.path.join("client-state").join("project-plan")),
        "the durable record lives in the existing client-state root, not a second location"
    );
    assert_eq!(Operation::ProjectRegister.as_str(), "project.register");
}

/// One canonical plan document a case submits through the real port.
fn plan_document(
    project_id: &str,
    plan_id: &str,
    work_items: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "schema": licoup_project::PLAN_DOCUMENT_SCHEMA,
        "projectId": project_id,
        "planId": plan_id,
        "source": {
            "sourceId": "source:roadmap",
            "sourceKind": "markdown",
            "locator": "docs/roadmap.md",
        },
        "workItems": work_items,
    })
}

/// One declared work item that admits cleanly.
fn plan_work_item(id: &str) -> serde_json::Value {
    serde_json::json!({
        "workItemId": id,
        "outcome": format!("Deliver {id}."),
        "acceptance": ["The declared outcome holds."],
        "inputs": [],
        "roles": [{"roleId": "role:maintainer", "scope": "work-item"}],
        "sourceAnchor": format!("heading:{id}"),
    })
}

/// A user entry and an Agent caller reach one import owner with one answer.
#[test]
fn an_import_previews_and_applies_through_the_port_the_client_composes() {
    let root = TempRoot::new("import");
    let application = NativeProjectApplication::at(&root.path);
    let claim = ActorClaim::local_admin("membership:owner");
    outcome(execute(
        &application,
        &claim,
        ProjectCommand::Register(request(
            "alpha-project",
            "workspace:shared",
            "plan:alpha",
            &root.declared_root("alpha"),
            "membership:owner",
        )),
    ));

    let document = plan_document(
        "alpha-project",
        "plan:alpha",
        serde_json::json!([plan_work_item("work:read"), plan_work_item("work:write")]),
    );
    let preview = outcome(execute(
        &application,
        &claim,
        ProjectCommand::ImportPreview {
            document: document.clone(),
        },
    ));
    assert_eq!(preview.reference.operation, "project.import.preview");
    assert_eq!(preview.payload["revision"], 0);
    assert_eq!(preview.payload["added"].as_array().map(Vec::len), Some(2));
    assert_eq!(preview.payload["applied"], serde_json::Value::Null);

    let applied = outcome(execute(
        &application,
        &claim,
        ProjectCommand::ImportApply {
            document: document.clone(),
            expected_revision: 0,
        },
    ));
    assert_eq!(applied.reference.operation, "project.import.apply");
    assert_eq!(applied.reference.id, "plan:alpha");
    assert_eq!(applied.reference.state, OperationState::Completed);
    assert_eq!(applied.payload["applied"], true);
    assert_eq!(applied.payload["change"]["revision"], 1);
    assert_eq!(
        applied.payload["change"]["mapping"][1]["sourceAnchor"],
        "heading:work:write"
    );

    // A replay is one effect: the same document does not apply twice.
    let replay = outcome(execute(
        &application,
        &claim,
        ProjectCommand::ImportApply {
            document,
            expected_revision: 1,
        },
    ));
    assert_eq!(replay.payload["applied"], false);
    assert_eq!(replay.payload["change"]["revision"], 1);

    // An Agent caller gets the same refusal a user gets, with the owner's code.
    let stale = failed(execute(
        &application,
        &claim,
        ProjectCommand::ImportApply {
            document: plan_document(
                "alpha-project",
                "plan:alpha",
                serde_json::json!([plan_work_item("work:read")]),
            ),
            expected_revision: 0,
        },
    ));
    assert_eq!(stale.code, "project_plan_import_stale_apply");
    assert_eq!(stale.stage, "project/import");
    assert!(!stale.retryable);

    // A document the owner cannot admit is refused with its own code and path.
    let mut progress = plan_work_item("work:read");
    progress["status"] = serde_json::json!("done");
    let ambiguous = failed(execute(
        &application,
        &claim,
        ProjectCommand::ImportPreview {
            document: plan_document("alpha-project", "plan:alpha", serde_json::json!([progress])),
        },
    ));
    assert_eq!(ambiguous.code, "project_plan_progress_not_admitted");
    assert_eq!(ambiguous.field.as_deref(), Some("workItems[0].status"));
}

fn dependency(
    project_id: &str,
    consumer: &str,
    producer: &str,
    path: &str,
) -> DependencyDeclarationRequest {
    DependencyDeclarationRequest {
        project_id: project_id.to_owned(),
        work_item_id: consumer.to_owned(),
        artifact: ArtifactInputRequest::Local {
            producer_work_item_id: producer.to_owned(),
            path: path.to_owned(),
        },
    }
}

/// A dependency is stored in the declaring project's index, so declaring one
/// needs that project's own authority. A caller that holds a different
/// membership cannot write into another project's declarations, and a caller
/// that names no registered project is refused before the store is reached.
#[test]
fn declaring_inputs_for_a_project_needs_that_projects_authority() {
    let root = TempRoot::new("dependency-authority");
    let application = NativeProjectApplication::at(&root.path);
    let foreign = ActorClaim::membership("codex", "conversation:one", "membership:codex");
    outcome(execute(
        &application,
        &foreign,
        ProjectCommand::Register(request(
            "alpha-project",
            "workspace:shared",
            "plan:alpha",
            &root.declared_root("alpha"),
            "membership:codex",
        )),
    ));

    let owner = ActorClaim::local_admin("membership:owner");
    let refusal = failed(execute(
        &application,
        &owner,
        ProjectCommand::DeclareDependency(dependency(
            "alpha-project",
            "test",
            "build",
            "dist/out.bin",
        )),
    ));
    assert_eq!(refusal.code, "project_dependency_authority_unauthorized");
    assert_eq!(refusal.stage, "project/dependency");
    assert!(!refusal.retryable);

    let listed = outcome(execute(
        &application,
        &owner,
        ProjectCommand::Dependencies {
            project_id: "alpha-project".to_owned(),
        },
    ));
    assert_eq!(
        listed.payload["dependencies"],
        serde_json::json!([]),
        "an unauthorized declaration leaves no row behind"
    );

    // The authority that registered the project declares its inputs.
    let declared = outcome(execute(
        &application,
        &foreign,
        ProjectCommand::DeclareDependency(dependency(
            "alpha-project",
            "test",
            "build",
            "dist/out.bin",
        )),
    ));
    assert_eq!(declared.reference.operation, "project.declare-dependency");
    assert_eq!(declared.reference.id, "alpha-project/test");
    assert_eq!(declared.reference.state, OperationState::Completed);
    assert_eq!(declared.payload["artifactState"], "missing");

    let refusal = failed(execute(
        &application,
        &foreign,
        ProjectCommand::DeclareDependency(dependency(
            "ghost-project",
            "test",
            "build",
            "dist/out.bin",
        )),
    ));
    assert_eq!(refusal.code, "project_dependency_project_unauthorized");
}

/// The actionable path of a refused cycle is published; a filesystem location
/// is not. The refusal's own code carries the reason, and only what a caller may
/// be shown travels beside it.
#[test]
fn a_refused_cycle_publishes_its_path_and_an_escaping_location_publishes_none() {
    let root = TempRoot::new("failure-projection");
    let application = NativeProjectApplication::at(&root.path);
    let claim = ActorClaim::local_admin("membership:owner");
    outcome(execute(
        &application,
        &claim,
        ProjectCommand::Register(request(
            "alpha-project",
            "workspace:shared",
            "plan:alpha",
            &root.declared_root("alpha"),
            "membership:owner",
        )),
    ));
    for (consumer, producer, path) in [
        ("lint", "build", "build/lint.log"),
        ("package", "lint", "dist/package.tar"),
    ] {
        outcome(execute(
            &application,
            &claim,
            ProjectCommand::DeclareDependency(dependency(
                "alpha-project",
                consumer,
                producer,
                path,
            )),
        ));
    }

    let cycle = failed(execute(
        &application,
        &claim,
        ProjectCommand::DeclareDependency(dependency(
            "alpha-project",
            "build",
            "package",
            "dist/rebuilt.tar",
        )),
    ));
    assert_eq!(cycle.code, "project_dependency_cycle");
    assert_eq!(
        cycle.presentation_args.get("dependencyPath"),
        Some(
            "alpha-project/build -> alpha-project/package -> alpha-project/lint -> alpha-project/build"
        )
    );

    let escape = failed(execute(
        &application,
        &claim,
        ProjectCommand::DeclareDependency(dependency(
            "alpha-project",
            "test",
            "build",
            "../outside/alpha-secret/leak.txt",
        )),
    ));
    assert_eq!(
        escape.code,
        "project_artifact_reference_escapes_authorized_root"
    );
    assert!(
        escape.presentation_args.is_empty(),
        "a location the caller declared is a diagnostic, not a public failure argument"
    );
}
