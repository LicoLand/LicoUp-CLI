//! The authorized-project command family.
//!
//! Nine routes, one registry: register one explicitly declared authorized
//! project, read one registered project, list the registered projects, preview
//! or apply one canonical plan document, declare one artifact input a work item
//! takes, and the three queries the declared dependency model exists for — the
//! declared inputs, the ones whose result is not materialized, and the consumers
//! a blocked producer actually blocks. Each route decodes its own arguments into
//! the shared [`licoup_application::ProjectCommand`] and runs it through the
//! single application facade, so the CLI, an authorized Agent caller and any
//! other interface reach one implementation with one refusal vocabulary.
//!
//! The registration payload is the declared identity set: project, workspace,
//! plan, authorized root, and the authority reference the caller registers
//! under. Nothing here resolves an identity from a directory.
//!
//! # Converting a source into a plan document
//!
//! `project import-preview` and `project import-apply` take the canonical plan
//! document on `--stdin-json`; an apply also takes the `--expected-revision` it
//! previewed. The caller — a person, a script or an authorized Agent — reads the
//! source under its own authorization and converts it deliberately. This owner
//! never opens, scans or parses a source document, and there is no model call
//! anywhere on this path.
//!
//! The document declares `schema` (`licoup.project-plan/v1`), `projectId`,
//! `planId`, the `source` it was read from and one or more `workItems`. Each work
//! item declares its `workItemId`, `outcome`, `acceptance` criteria, `inputs`,
//! `roles` and the `sourceAnchor` it was read from. Two properties are not
//! negotiable, and both are refusals rather than repairs:
//!
//! - **Progress is not admitted.** A `status`, `progress`, `completedAt`,
//!   `accepted` or similar field is refused by code
//!   (`project_plan_progress_not_admitted`) and by path. A run, a completion and
//!   an acceptance are established by the work owner that observed them; an
//!   import can never mark work executed or accepted.
//! - **An omission is not a deletion.** A work item the document leaves out is
//!   retained and reported as `retained`, never deleted or cancelled.
//!
//! A preview changes nothing and returns the revision an apply must expect. An
//! apply whose `--expected-revision` is not current is refused
//! (`project_plan_import_stale_apply`); re-submitting the document already
//! stored is one effect, not two. An import never starts execution and never
//! widens a directory, cost, disclosure or task grant.
//!
//! No route composes the store's location: the port resolves the durable store
//! through the layout owner that already names the client-state root, so this
//! surface never joins a directory name of its own. The client bridge family for
//! these commands is not owned here either — `PROJECT-COMMAND-COMPOSITION` owns
//! the generated bridge sides (`schemas/client_bridge/project.json`).

use super::{AdmittedCommand, CliExecution, handler_error};
use crate::domain::application_port;
use anyhow::Result;
use licoup_application::{
    ApplicationCommand, ApplicationFailure, CommandResolution, DependencyDeclarationRequest,
    ProjectCommand, ProjectRegistrationRequest,
};
use serde_json::{Value, json};

/// Stable identifier of the envelope this surface publishes.
const PROJECT_ENVELOPE_SCHEMA: &str = "licoup.project-identity/v1";

pub(super) fn handle_project_register(command: AdmittedCommand) -> Result<CliExecution> {
    let payload = command
        .option_json("stdin-json")
        .cloned()
        .ok_or_else(|| handler_error("cli_json_invalid", "provide_valid_json"))?;
    let request = ProjectRegistrationRequest::decode(&payload)
        .map_err(|_| handler_error("cli_json_invalid", "provide_valid_json"))?;
    Ok(project_surface(ProjectCommand::Register(request)))
}

pub(super) fn handle_project_read(command: AdmittedCommand) -> Result<CliExecution> {
    Ok(project_surface(ProjectCommand::Read {
        project_id: command.required_text("project-id").to_owned(),
    }))
}

pub(super) fn handle_project_list(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(project_surface(ProjectCommand::List))
}

pub(super) fn handle_project_import_preview(command: AdmittedCommand) -> Result<CliExecution> {
    let document = command
        .option_json("stdin-json")
        .cloned()
        .ok_or_else(|| handler_error("cli_json_invalid", "provide_valid_json"))?;
    Ok(project_surface(ProjectCommand::ImportPreview { document }))
}

pub(super) fn handle_project_import_apply(command: AdmittedCommand) -> Result<CliExecution> {
    let expected_revision = command
        .option_text("expected-revision")
        .ok_or_else(|| handler_error("cli_argument_invalid", "provide_expected_revision"))?
        .parse::<u64>()
        .map_err(|_| handler_error("cli_argument_invalid", "provide_expected_revision"))?;
    let document = command
        .option_json("stdin-json")
        .cloned()
        .ok_or_else(|| handler_error("cli_json_invalid", "provide_valid_json"))?;
    Ok(project_surface(ProjectCommand::ImportApply {
        document,
        expected_revision,
    }))
}

/// Declare one artifact input: the consumer work item and the result it takes.
///
/// The declaration names the producer inside the artifact, so a payload cannot
/// name one producer and take the result of another.
pub(super) fn handle_project_dependency_declare(command: AdmittedCommand) -> Result<CliExecution> {
    let payload = command
        .option_json("stdin-json")
        .cloned()
        .ok_or_else(|| handler_error("cli_json_invalid", "provide_valid_json"))?;
    let request = DependencyDeclarationRequest::decode(&payload)
        .map_err(|_| handler_error("cli_json_invalid", "provide_valid_json"))?;
    Ok(project_surface(ProjectCommand::DeclareDependency(request)))
}

/// Every dependency one project declares, each with its explicit artifact state.
pub(super) fn handle_project_dependency_list(command: AdmittedCommand) -> Result<CliExecution> {
    Ok(project_surface(ProjectCommand::Dependencies {
        project_id: command.required_text("project-id").to_owned(),
    }))
}

/// Every declared reference of one project whose result is not materialized.
pub(super) fn handle_project_dependency_unresolved(
    command: AdmittedCommand,
) -> Result<CliExecution> {
    Ok(project_surface(ProjectCommand::UnresolvedArtifacts {
        project_id: command.required_text("project-id").to_owned(),
    }))
}

/// The consumers one blocked producer blocks, transitively, across projects.
pub(super) fn handle_project_dependency_blocked(command: AdmittedCommand) -> Result<CliExecution> {
    Ok(project_surface(ProjectCommand::BlockedConsumers {
        project_id: command.required_text("project-id").to_owned(),
        work_item_id: command.required_text("work-item-id").to_owned(),
    }))
}

/// Run one project command through the shared facade and frame its resolution.
fn project_surface(command: ProjectCommand) -> CliExecution {
    let mut envelope = json!({
        "schema": PROJECT_ENVELOPE_SCHEMA,
        "family": "project",
        "operation": command.operation().as_str(),
    });
    match application_port::execute(&ApplicationCommand::Project(command)) {
        CommandResolution::Resolved(outcome) => {
            envelope["status"] = Value::String("ok".to_owned());
            envelope["outcome"] = serde_json::to_value(outcome).unwrap_or(Value::Null);
        }
        CommandResolution::Failed(failure) => {
            envelope["status"] = Value::String("failed".to_owned());
            envelope["failure"] = failure_wire(&failure);
        }
    }
    CliExecution::Json(envelope)
}

/// The failure body: the neutral fields, with the caller's own code and stage.
///
/// The one extra field is the public presentation arguments a refusal carries —
/// the work-item path of a refused dependency cycle — and only when the owner
/// published one. A failure with nothing public to say stays exactly as it was.
fn failure_wire(failure: &ApplicationFailure) -> Value {
    let mut body = json!({
        "code": failure.code,
        "stage": failure.stage,
        "retryable": failure.retryable,
        "recovery": failure.recovery.mcp_wire(),
    });
    if !failure.presentation_args.is_empty() {
        body["presentationArgs"] =
            serde_json::to_value(&failure.presentation_args).unwrap_or(Value::Null);
    }
    body
}
