//! The typed Conversation surface: `licoup conversation list|get`.
//!
//! These two commands are the first slice of the application surface. Unlike
//! `conversation execute` they take no JSON body on argv: the request is built
//! from typed options and handed to the shared `licoup_application` facade,
//! which is the one place that reaches the durable conversation host.
//!
//! # Envelope
//!
//! Both commands print exactly one JSON object, and nothing else, on stdout.
//! The shape is versioned and belongs to this surface alone; it is distinct
//! from the NDJSON stdio-RPC frames (`protocol`/`id`/`workflowId`/`ok`).
//!
//! A resolved read:
//!
//! ```json
//! {
//!   "schema": "licoup.application-envelope/v1",
//!   "version": 1,
//!   "family": "conversation",
//!   "operation": "conversation.list",
//!   "status": "ok",
//!   "outcome": {
//!     "reference": {
//!       "operation": "conversation.list",
//!       "id": "",
//!       "state": "completed"
//!     },
//!     "payload": { "...": "the operation's own body" }
//!   }
//! }
//! ```
//!
//! A refused command replaces `outcome` with the normalized failure:
//!
//! ```json
//! {
//!   "schema": "licoup.application-envelope/v1",
//!   "version": 1,
//!   "family": "conversation",
//!   "operation": "conversation.get",
//!   "status": "failed",
//!   "failure": {
//!     "code": "invalid_request",
//!     "stage": "schema/validate",
//!     "retryable": false,
//!     "effect": "not-attempted",
//!     "recovery": "correct_request",
//!     "field": "conversation_id"
//!   }
//! }
//! ```
//!
//! `status` is `"ok"` or `"failed"`, and `outcome` and `failure` are mutually
//! exclusive because a command either resolved or failed. `failure.recovery` is
//! the client projection of `RecoveryAction::cli_wire`; `failure.field` is
//! present only when the failure names one.

use super::{AdmittedCommand, CliExecution};
use crate::domain::application_port;
use anyhow::Result;
use licoup_application::{
    ApplicationCommand, ApplicationFailure, CommandFamily, CommandResolution, ConversationCommand,
};
use serde_json::{Value, json};

/// Stable identifier of the envelope this surface publishes.
pub(super) const APPLICATION_ENVELOPE_SCHEMA: &str = "licoup.application-envelope/v1";
/// Version of the envelope this surface publishes.
pub(super) const APPLICATION_ENVELOPE_VERSION: u64 = 1;

pub(super) fn handle_conversation_list(command: AdmittedCommand) -> Result<CliExecution> {
    let include_archived = command.option_flag("include-archived");
    Ok(conversation_surface(ConversationCommand::List {
        include_archived,
    }))
}

pub(super) fn handle_conversation_get(command: AdmittedCommand) -> Result<CliExecution> {
    let conversation_id = command
        .option_text("conversation-id")
        .unwrap_or_default()
        .to_owned();
    Ok(conversation_surface(ConversationCommand::Get {
        conversation_id,
    }))
}

/// Run one Conversation command through the shared facade and frame the result.
fn conversation_surface(command: ConversationCommand) -> CliExecution {
    CliExecution::Json(application_envelope(ApplicationCommand::Conversation(
        command,
    )))
}

/// Project one resolution into the single versioned envelope of this surface.
fn application_envelope(command: ApplicationCommand) -> Value {
    let mut envelope = json!({
        "schema": APPLICATION_ENVELOPE_SCHEMA,
        "version": APPLICATION_ENVELOPE_VERSION,
        "family": family_wire(command.family()),
        "operation": command.operation().as_str(),
    });
    match application_port::execute(&command) {
        CommandResolution::Resolved(outcome) => {
            envelope["status"] = Value::String("ok".to_owned());
            envelope["outcome"] = serde_json::to_value(outcome).unwrap_or(Value::Null);
        }
        CommandResolution::Failed(failure) => {
            envelope["status"] = Value::String("failed".to_owned());
            envelope["failure"] = failure_wire(&failure);
        }
    }
    envelope
}

/// The failure body: the neutral fields, with recovery in the client vocabulary.
fn failure_wire(failure: &ApplicationFailure) -> Value {
    let effect = serde_json::to_value(failure.effect).unwrap_or(Value::Null);
    let mut body = json!({
        "code": &failure.code,
        "stage": &failure.stage,
        "retryable": failure.retryable,
        "effect": effect,
        "recovery": failure.recovery.cli_wire(),
    });
    if let Some(field) = &failure.field {
        body["field"] = Value::String(field.clone());
    }
    body
}

/// The family name the envelope publishes.
fn family_wire(family: CommandFamily) -> &'static str {
    match family {
        CommandFamily::Assistant => "assistant",
        CommandFamily::Subagent => "subagent",
        CommandFamily::Conversation => "conversation",
        CommandFamily::Project => "project",
    }
}

#[cfg(test)]
mod tests {
    use super::super::{CliCommandError, CliExecution, admit_cli_command, execute_cli};
    use super::APPLICATION_ENVELOPE_SCHEMA;

    /// A malformed `--conversation-id` is refused before any port runs, and the
    /// refusal is the typed failure envelope rather than a bare process error.
    #[test]
    fn malformed_conversation_id_is_refused_as_typed_json_failure() {
        let execution = execute_cli(vec![
            "conversation".to_owned(),
            "get".to_owned(),
            "--conversation-id".to_owned(),
            "   ".to_owned(),
        ])
        .expect("the typed surface returns a JSON envelope, not a bare error");
        let CliExecution::Json(envelope) = execution else {
            panic!("conversation get must print exactly one JSON envelope");
        };
        assert_eq!(envelope["schema"], APPLICATION_ENVELOPE_SCHEMA);
        assert_eq!(envelope["version"], 1);
        assert_eq!(envelope["family"], "conversation");
        assert_eq!(envelope["operation"], "conversation.get");
        assert_eq!(envelope["status"], "failed");
        let failure = &envelope["failure"];
        assert_eq!(failure["code"], "invalid_request");
        assert_eq!(failure["stage"], "schema/validate");
        assert_eq!(failure["retryable"], false);
        assert_eq!(failure["effect"], "not-attempted");
        assert_eq!(failure["recovery"], "correct_request");
        assert_eq!(failure["field"], "conversation_id");
        assert!(
            envelope.get("outcome").is_none(),
            "a failed command must not also carry an outcome"
        );
    }

    /// The typed surface has no body channel: the request body is built in
    /// process and can never be supplied — or leaked — through argv.
    #[test]
    fn the_typed_surface_never_accepts_a_request_body_on_argv() {
        let error = admit_cli_command(vec![
            "conversation".to_owned(),
            "get".to_owned(),
            "--conversation-id".to_owned(),
            "conversation:one".to_owned(),
            "--stdin-json".to_owned(),
            r#"{"action":"conversation.get"}"#.to_owned(),
        ])
        .expect_err("a body-bearing argv must be refused");
        let typed = error
            .downcast_ref::<CliCommandError>()
            .expect("the refusal must stay a typed admission error");
        assert_eq!(typed.code(), "cli_option_unknown");
    }
}
