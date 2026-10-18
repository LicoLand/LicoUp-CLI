//! Authorized project and plan identity, the dependency inputs a project
//! declares, the canonical plan document an explicit import carries, the
//! scheduling answer that work is read against, and the preview of a declared
//! change over them.
//!
//! This crate owns one thing: the explicit record that says a project exists,
//! who authorized it, where its authorized root is, which workspace and plan
//! identity it carries, and which declared results its work items take from
//! other work items. A project is *registered*, never discovered — there is no
//! directory walk anywhere below this paragraph, and
//! [`ProjectIdentitySource::Discovered`] exists only so that a request asking
//! the owner to name a project by reading its root is refused with a code
//! instead of being answered with a guess.
//!
//! Five rules make the record trustworthy rather than descriptive:
//!
//! - **Identity is declared.** A [`ProjectId`], [`WorkspaceId`], [`PlanId`] and
//!   [`WorkItemId`] are caller-supplied bounded identifiers, not derived from a
//!   path or a directory listing. [`AuthorizedRoot`] is a declared absolute
//!   location the owner stores but never opens to name a project.
//! - **Authority is a reference.** A registration names an
//!   [`AuthorityReference`] into the authority owner that already exists
//!   ([`ProjectAuthorityDirectory`]); no credential, token or secret is copied
//!   into the record, and the persisted table has no column that could hold one.
//! - **Dependencies are declared inputs.** A [`WorkDependency`] names the
//!   consumer work item and the result it takes, either as an
//!   [`ArtifactReference::Local`] location inside the declaring project's
//!   authorized root or as an [`ArtifactReference::CrossProject`] reference to a
//!   work item of another registered project. A shared result is referenced, not
//!   produced twice.
//! - **A change is previewed, never applied.**
//!   [`ProjectIdentityStore::preview_change`] answers what a [`ChangeRequest`]
//!   would touch by following the declared edges, and asks the existing work
//!   owner ([`WorkActivityDirectory`]) what sits behind the declaration it
//!   replaces. It writes nothing, cancels nothing and rewrites no acceptance: a
//!   run in flight or an accepted result is reported as requiring an explicit
//!   handoff through the existing authority, and an owner that does not answer
//!   leaves the work item unresolved rather than fresh.
//! - **Refusal is typed, and so is absence.** A duplicate identity, a duplicate
//!   plan identity, an absent or unauthorized reference, an identity that would
//!   need a directory scan, a dependency cycle, an unauthorized cross-project
//!   reference and a location that escapes its authorized root all return a
//!   [`ProjectFailure`] code, so both interfaces report the same reason for the
//!   same request. An artifact that is not there is not an error and not an
//!   empty result: it is reported as [`ArtifactState::Missing`] or
//!   [`ArtifactState::Unavailable`], explicitly, and never resolved by walking
//!   outside the declared roots.
//!
//! The import boundary is the fifth rule, and it is deliberately narrow: an
//! arbitrary source document is converted by a caller into one canonical
//! [`PlanDocument`] whose work items declare an outcome, acceptance criteria,
//! inputs, role references and the source anchor they were read from. A
//! declaration is never a fact — the model has no field for a run, a completion
//! or an acceptance, a document that carries one is refused by path
//! (`project_plan_progress_not_admitted`), and [`PlanDocument::admit`] resolves
//! the document's own references and returns the source correspondence before
//! anything is stored.
//!
//! Durability reuses the existing state root: the store writes one SQLite
//! database inside the client-state root, by the same location rule the
//! workflow store follows, with the same private-directory, schema-version and
//! connection rules. Identities and dependency inputs share that one database.
//! A preview adds no table and no row: everything it answers is derived from
//! declarations that are already durable.

mod activity;
mod authority;
mod change;
mod dependency;
mod failure;
mod identity;
mod import;
mod schedule;
mod store;

pub use activity::{NoWorkActivityDirectory, WorkActivity, WorkActivityDirectory};
pub use authority::{NoAuthorityDirectory, ProjectAuthorityDirectory};
pub use change::{
    AffectedWorkItem, ChangeHandoff, ChangeImpact, ChangePreview, ChangeRequest,
    DeclaredInputState, MAX_CHANGE_INPUTS, MAX_CHANGE_WORK_ITEMS, WorkItemChange,
};
pub use dependency::{
    ArtifactReference, ArtifactState, DeclaredDependency, MAX_ARTIFACT_PATH_BYTES, WorkDependency,
    WorkRef, read_local_artifact, render_dependency_path, stays_inside_authorized_root,
};
pub use failure::{
    CHANGE_STAGE, DEPENDENCY_STAGE, IDENTITY_STAGE, IMPORT_STAGE, ProjectFailure,
    REGISTRATION_STAGE, SCHEDULE_STAGE, STORE_STAGE,
};
pub use identity::{
    AuthorityKind, AuthorityReference, AuthorizedRoot, MAX_AUTHORITY_REFERENCE_BYTES,
    MAX_AUTHORIZED_ROOT_BYTES, MAX_DISPLAY_NAME_BYTES, MAX_PLAN_ID_BYTES, MAX_PROJECT_ID_BYTES,
    MAX_WORK_ITEM_ID_BYTES, MAX_WORKSPACE_ID_BYTES, PlanId, ProjectId, ProjectIdentitySource,
    ProjectRegistration, RegisteredProject, WorkItemId, WorkspaceId,
};
pub use import::{
    CapabilityId, IMPORT_PLAN_MISMATCH, IMPORT_PROJECT_UNAUTHORIZED, IMPORT_RECORD_INVALID,
    IMPORT_STALE_APPLY, ImportDiagnostic, ImportSlice, MAX_PLAN_ACCEPTANCE, MAX_PLAN_INPUTS,
    MAX_PLAN_ROLES, MAX_PLAN_TEXT_BYTES, MAX_PLAN_WORK_ITEMS, MAX_SOURCE_LOCATOR_BYTES,
    PLAN_DOCUMENT_SCHEMA, PLAN_IMPORT_STAGE, PlanAdmission, PlanDocument, PlanImportChange,
    PlanImportOutcome, PlanWorkItem, RoleId, RoleReference, RoleScope, SourceId, SourceIdentity,
    SourceKind, SourceLocator, SourceMapping,
};
pub use schedule::{
    BlockedWork, OutstandingWork, SCHEDULE_PROJECT_UNAUTHORIZED, StopScope, WorkReadiness,
};
pub use store::{
    PROJECT_DEPENDENCY_COLUMNS, PROJECT_IDENTITY_COLUMNS, PROJECT_IMPORT_SOURCE_COLUMNS,
    PROJECT_PLAN_IMPORT_COLUMNS, PROJECT_STORE_SCHEMA_VERSION, ProjectIdentityStore,
};

#[cfg(test)]
mod tests;
