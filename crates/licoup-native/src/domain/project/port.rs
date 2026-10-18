//! The project family behind the shared `licoup_application` facade.
//!
//! One translation each way: the typed command becomes the crate owner's own
//! registration or query, and the owner's answer becomes the neutral
//! [`CommandOutcome`] both interfaces already publish. The port decides nothing
//! about identity, authority or durability — it supplies the authority answer
//! and the store, and refuses a registration the caller's own claim cannot
//! authorize before the store is reached.
//!
//! The store is opened per call, exactly as the workflow store opens its own
//! connection: a one-shot CLI process and the durable desktop host then read the
//! same rows, and a data root that is unavailable becomes a typed retryable
//! failure instead of a facade that cannot be composed.

use super::authority::VerifiedClaimAuthority;
use crate::domain::project::{portable_store, store_at};
use licoup_application::{
    ActorClaim, ApplicationFailure, ArtifactInputRequest, CommandOutcome,
    DependencyDeclarationRequest, FailureNormalization, Operation, OperationReference,
    OperationState, ProjectCommand, ProjectPort, ProjectRegistrationRequest,
};
use licoup_project::{
    ArtifactReference, AuthorityKind, AuthorityReference, DeclaredDependency, ImportDiagnostic,
    PLAN_IMPORT_STAGE, PlanAdmission, PlanDocument, ProjectFailure, ProjectId,
    ProjectIdentitySource, ProjectIdentityStore, ProjectRegistration, RegisteredProject,
    WorkDependency, WorkItemId, WorkRef,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Which portable data root this family composes over.
#[derive(Clone, Debug)]
enum ProjectStoreRoot {
    /// The data root this process selected.
    Portable,
    /// An explicit root, used by tests and by an embedding host.
    At(PathBuf),
}

/// The project identity family, composed over one durable store root.
pub struct NativeProjectApplication {
    root: ProjectStoreRoot,
}

impl NativeProjectApplication {
    /// Compose the family over a store rooted at one portable data root.
    pub fn at(portable_root: &Path) -> Self {
        Self {
            root: ProjectStoreRoot::At(portable_root.to_path_buf()),
        }
    }

    /// Compose the family over this process's portable data root.
    pub fn portable() -> Self {
        Self {
            root: ProjectStoreRoot::Portable,
        }
    }

    /// Open the durable store this family reads and writes.
    pub fn store(&self) -> Result<ProjectIdentityStore, ProjectFailure> {
        match &self.root {
            ProjectStoreRoot::Portable => portable_store(),
            ProjectStoreRoot::At(root) => store_at(root),
        }
    }
}

impl ProjectPort for NativeProjectApplication {
    fn execute(
        &self,
        claim: &ActorClaim,
        command: &ProjectCommand,
    ) -> Result<CommandOutcome, ApplicationFailure> {
        match command {
            ProjectCommand::Register(request) => self.register(claim, request),
            ProjectCommand::Read { project_id } => {
                let project_id = ProjectId::declare(project_id.clone()).map_err(failure)?;
                let project = self
                    .store()
                    .map_err(failure)?
                    .read(&project_id)
                    .map_err(failure)?;
                Ok(read_outcome(
                    Operation::ProjectRead,
                    json!({"project": project.as_ref().map(project_wire)}),
                ))
            }
            ProjectCommand::List => {
                let projects = self
                    .store()
                    .map_err(failure)?
                    .list()
                    .map_err(failure)?
                    .iter()
                    .map(project_wire)
                    .collect::<Vec<_>>();
                Ok(read_outcome(
                    Operation::ProjectList,
                    json!({"projects": projects}),
                ))
            }
            ProjectCommand::ImportPreview { document } => {
                let admission = admit_document(document.clone())?;
                let change = self
                    .store()
                    .map_err(failure)?
                    .preview_import(&admission)
                    .map_err(failure)?;
                Ok(read_outcome(
                    Operation::ProjectImportPreview,
                    serde_json::to_value(change).unwrap_or(Value::Null),
                ))
            }
            ProjectCommand::ImportApply {
                document,
                expected_revision,
            } => {
                let admission = admit_document(document.clone())?;
                let outcome = self
                    .store()
                    .map_err(failure)?
                    .apply_import(&admission, *expected_revision)
                    .map_err(failure)?;
                Ok(CommandOutcome::new(OperationReference::new(
                    Operation::ProjectImportApply,
                    admission.document.plan_id.as_str(),
                    OperationState::Completed,
                ))
                .with_payload(serde_json::to_value(outcome).unwrap_or(Value::Null)))
            }
            ProjectCommand::DeclareDependency(request) => self.declare_dependency(claim, request),
            ProjectCommand::Dependencies { project_id } => {
                let declared = self
                    .store()
                    .map_err(failure)?
                    .dependencies(&declared_project(project_id).map_err(failure)?)
                    .map_err(failure)?;
                Ok(read_outcome(
                    Operation::ProjectDependencies,
                    json!({
                        "projectId": project_id,
                        "dependencies": declared.iter().map(dependency_wire).collect::<Vec<_>>(),
                    }),
                ))
            }
            ProjectCommand::UnresolvedArtifacts { project_id } => {
                let unresolved = self
                    .store()
                    .map_err(failure)?
                    .unresolved_artifacts(&declared_project(project_id).map_err(failure)?)
                    .map_err(failure)?;
                Ok(read_outcome(
                    Operation::ProjectUnresolvedArtifacts,
                    json!({
                        "projectId": project_id,
                        "unresolvedArtifacts": unresolved.iter().map(dependency_wire).collect::<Vec<_>>(),
                    }),
                ))
            }
            ProjectCommand::BlockedConsumers {
                project_id,
                work_item_id,
            } => {
                let producer = WorkRef::new(
                    declared_project(project_id).map_err(failure)?,
                    declared_item(work_item_id).map_err(failure)?,
                );
                let blocked = self
                    .store()
                    .map_err(failure)?
                    .blocked_consumers(&producer)
                    .map_err(failure)?;
                Ok(read_outcome(
                    Operation::ProjectBlockedConsumers,
                    json!({
                        "producer": work_ref_wire(&producer),
                        "blockedConsumers": blocked.iter().map(work_ref_wire).collect::<Vec<_>>(),
                    }),
                ))
            }
        }
    }
}

/// Parse and resolve one carried document, or refuse with its own diagnostics.
///
/// The document crosses the facade as an opaque value because this facade is
/// protocol-neutral; it becomes the owner's typed declaration here, once, and
/// every refusal it produces is reported before the store is reached. The
/// published code is the first diagnostic's — a caller branches on one stable
/// code — and its document path travels as the offending field, so an ambiguous
/// conversion is corrected at the exact place the owner named.
fn admit_document(document: Value) -> Result<PlanAdmission, ApplicationFailure> {
    let document = PlanDocument::from_value(document).map_err(import_diagnostics)?;
    document.admit().map_err(import_diagnostics)
}

/// One refusal for a document the owner would not admit.
fn import_diagnostics(diagnostics: Vec<ImportDiagnostic>) -> ApplicationFailure {
    let first = diagnostics.first();
    let failure = ApplicationFailure::permanent(
        first.map_or("project_plan_document_invalid", |first| first.code),
        PLAN_IMPORT_STAGE,
    );
    match first.map(|first| first.path.as_str()) {
        Some(path) if !path.is_empty() => failure.with_field(path),
        _ => failure,
    }
}

impl NativeProjectApplication {
    fn register(
        &self,
        claim: &ActorClaim,
        request: &ProjectRegistrationRequest,
    ) -> Result<CommandOutcome, ApplicationFailure> {
        let authority = VerifiedClaimAuthority::of(claim).map_err(failure)?;
        let kind = AuthorityKind::parse(&request.authority_kind).ok_or_else(|| {
            failure(ProjectFailure::registration(
                "project_authority_reference_required",
            ))
        })?;
        let declared = AuthorityReference::declare(kind, request.authority_reference.clone())
            .map_err(failure)?;
        if &declared != authority.reference() {
            // The caller asked to register under an authority it does not hold.
            // Refusing here keeps the store's own admission one testable rule.
            return Err(failure(ProjectFailure::registration(
                "project_authority_unauthorized",
            )));
        }
        let registration = ProjectRegistration {
            identity: ProjectIdentitySource::Declared {
                project_id: request.project_id.clone(),
            },
            display_name: request.display_name.clone(),
            authorized_root: request.authorized_root.clone(),
            authority: Some(declared),
            workspace_id: request.workspace_id.clone(),
            plan_id: request.plan_id.clone(),
        };
        let registered = self
            .store()
            .map_err(failure)?
            .register(&authority, &registration)
            .map_err(failure)?;
        Ok(CommandOutcome::new(OperationReference::new(
            Operation::ProjectRegister,
            registered.project_id.as_str(),
            OperationState::Completed,
        ))
        .with_payload(project_wire(&registered)))
    }

    /// Admit one declared artifact input on behalf of an authorized caller.
    ///
    /// Two authorization rules run before the store admits the edge, and both
    /// are decisions this port owns rather than rules the store restates: the
    /// caller must hold the authority the *declaring* project was registered
    /// under, because the edge is stored in that project's index. A
    /// cross-project reference does not need the referenced project's authority
    /// — naming a registered project is the disclosure check the owner already
    /// makes — so a shared result can be referenced without a second grant.
    fn declare_dependency(
        &self,
        claim: &ActorClaim,
        request: &DependencyDeclarationRequest,
    ) -> Result<CommandOutcome, ApplicationFailure> {
        let dependency = declared_dependency(request).map_err(failure)?;
        let store = self.store().map_err(failure)?;
        let caller = claim
            .membership_id()
            .and_then(|membership_id| AuthorityReference::membership(membership_id).ok())
            .ok_or_else(|| {
                failure(ProjectFailure::dependency(
                    "project_dependency_authority_unauthorized",
                ))
            })?;
        match store.read(&dependency.project_id).map_err(failure)? {
            Some(project) if project.authority == caller => {}
            // A registered project under another authority and an unregistered
            // project are both refused before the store's own admission, so a
            // caller cannot write into an index it does not own.
            Some(_) => {
                return Err(failure(ProjectFailure::dependency(
                    "project_dependency_authority_unauthorized",
                )));
            }
            None => {
                return Err(failure(ProjectFailure::dependency(
                    "project_dependency_project_unauthorized",
                )));
            }
        }
        let declared = store.admit_dependency(&dependency).map_err(failure)?;
        Ok(CommandOutcome::new(OperationReference::new(
            Operation::ProjectDeclareDependency,
            declared.consumer().to_string(),
            OperationState::Completed,
        ))
        .with_payload(dependency_wire(&declared)))
    }
}

/// One registered project as both interfaces publish it.
///
/// The authority travels as the reference the owner admitted, in its two parts;
/// there is no field for a credential, because the record has none.
fn project_wire(project: &RegisteredProject) -> Value {
    json!({
        "projectId": project.project_id.as_str(),
        "displayName": project.display_name,
        "authorizedRoot": project.authorized_root.as_str(),
        "authorityKind": project.authority.kind().as_str(),
        "authorityReference": project.authority.reference(),
        "workspaceId": project.workspace_id.as_str(),
        "planId": project.plan_id.as_str(),
        "registrationSequence": project.registration_sequence,
    })
}

/// A read outcome: no live identity, the operation's own body.
fn read_outcome(operation: Operation, payload: Value) -> CommandOutcome {
    CommandOutcome::new(OperationReference::new(
        operation,
        "",
        OperationState::Completed,
    ))
    .with_payload(payload)
}

/// One declared dependency as both interfaces publish it.
///
/// The declared reference travels exactly as the caller declared it, beside the
/// explicit state read from the declared roots. No field here is a resolution:
/// a missing result is published as `missing` rather than as an absent entry.
fn dependency_wire(declared: &DeclaredDependency) -> Value {
    json!({
        "dependencySequence": declared.dependency_sequence,
        "consumer": work_ref_wire(&declared.consumer()),
        "producer": work_ref_wire(&declared.producer()),
        "artifact": artifact_wire(&declared.dependency.artifact),
        "artifactState": declared.artifact_state.as_str(),
    })
}

/// One work item reference, project-qualified because an identity is unique
/// inside its project rather than globally.
fn work_ref_wire(work: &WorkRef) -> Value {
    json!({
        "projectId": work.project_id.as_str(),
        "workItemId": work.work_item_id.as_str(),
    })
}

/// The declared artifact reference, in the shape the declaration used.
fn artifact_wire(artifact: &ArtifactReference) -> Value {
    match artifact {
        ArtifactReference::Local {
            producer_work_item_id,
            path,
        } => json!({
            "kind": "local",
            "producerWorkItemId": producer_work_item_id.as_str(),
            "path": path,
        }),
        ArtifactReference::CrossProject {
            project_id,
            work_item_id,
        } => json!({
            "kind": "cross-project",
            "projectId": project_id.as_str(),
            "workItemId": work_item_id.as_str(),
        }),
    }
}

/// One bounded request as the owner's own declaration.
///
/// The surface bounds the fields; the owner's identity and location alphabets
/// are stricter, so an unusable identity is refused here with the owner's own
/// code rather than re-spelled as a second vocabulary.
fn declared_dependency(
    request: &DependencyDeclarationRequest,
) -> Result<WorkDependency, ProjectFailure> {
    let artifact = match &request.artifact {
        ArtifactInputRequest::Local {
            producer_work_item_id,
            path,
        } => ArtifactReference::local(declared_item(producer_work_item_id)?, path.clone())?,
        ArtifactInputRequest::CrossProject {
            project_id,
            work_item_id,
        } => ArtifactReference::cross_project(
            declared_project(project_id)?,
            declared_item(work_item_id)?,
        ),
    };
    Ok(WorkDependency {
        project_id: declared_project(&request.project_id)?,
        work_item_id: declared_item(&request.work_item_id)?,
        artifact,
    })
}

fn declared_project(project_id: &str) -> Result<ProjectId, ProjectFailure> {
    ProjectId::declare(project_id.to_owned())
}

fn declared_item(work_item_id: &str) -> Result<WorkItemId, ProjectFailure> {
    WorkItemId::declare(work_item_id.to_owned())
}

/// A project refusal in the neutral failure model.
///
/// An unavailable store is the one failure a caller may retry unchanged; every
/// other refusal names a request the caller has to correct, so it is permanent
/// rather than a blind retry.
///
/// The actionable path of a refused cycle is the one detail that travels: it is
/// the work-item path a caller has to break, and it is a public display value
/// built from identities the caller declared. Every other detail stays with the
/// owner, because it is a diagnostic — an escaping location's detail names
/// filesystem paths, which are not public failure arguments.
pub(super) fn failure(error: ProjectFailure) -> ApplicationFailure {
    let normalization = if error.code() == "project_identity_store_unavailable" {
        FailureNormalization::RETRYABLE
    } else {
        FailureNormalization::PERMANENT
    };
    let failure = normalization.into_failure(error.code(), error.stage());
    match error.detail() {
        Some(path) if error.code() == "project_dependency_cycle" => {
            failure.with_presentation_arg("dependencyPath", path)
        }
        _ => failure,
    }
}
