//! The local Subagents surface: `licoup subagents catalog|execute`.
//!
//! `execute` is the lane the desktop, the agents and the independent MCP process
//! all reach delegated work through. It decodes one admitted tool invocation
//! into a [`licoup_application`] command plus the caller claim it is made under,
//! and hands both to the shared facade. Nothing here reaches a domain owner:
//! there is one door, and this surface is one of the callers of it.
//!
//! The invocation envelope (`name`/`arguments`/`caller`) is this surface's own
//! wire shape, so it is decoded here and answered with the domain owner's own
//! receipt, or with the subagent error envelope when the facade refused.

use super::{AdmittedCommand, CliExecution};
use crate::domain::application_port;
use crate::domain::subagents::validate_tool_arguments;
use anyhow::{Result, anyhow};
use licoup_application::{
    ActorClaim, ApplicationCommand, ApplicationFailure, AssistantCommand, CallbackDecision,
    CancelRequest, CommandResolution, DispatchRequest, SubagentCommand, TaskType,
};
use serde_json::{Map, Value, json};

/// Stable identifier of the error envelope this surface publishes.
const SUBAGENT_ERROR_SCHEMA: &str = "licoup.subagent.error.v1";

pub(super) fn handle_subagents_catalog(_: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(application_port::subagent_catalog()?))
}

pub(super) fn handle_subagents_execute(mut command: AdmittedCommand) -> Result<CliExecution> {
    let input = command
        .take_option_json("stdin-json")
        .ok_or_else(|| anyhow!("subagents_request_required"))?;
    Ok(CliExecution::Json(execute_invocation(input)?))
}

/// The lifecycle of the independent MCP process this surface supervises.
fn lifecycle(action: &str, binary: Option<&str>) -> Result<CliExecution> {
    let binary = binary.map(std::path::PathBuf::from);
    Ok(CliExecution::Json(
        crate::platform::mcp_service_process::execute(action, binary.as_deref())?,
    ))
}
pub(super) fn handle_mcp_start(command: AdmittedCommand) -> Result<CliExecution> {
    lifecycle("start", command.option_text("binary"))
}
pub(super) fn handle_mcp_stop(_: AdmittedCommand) -> Result<CliExecution> {
    lifecycle("stop", None)
}
pub(super) fn handle_mcp_reload(command: AdmittedCommand) -> Result<CliExecution> {
    lifecycle("reload", command.option_text("binary"))
}
pub(super) fn handle_mcp_status(_: AdmittedCommand) -> Result<CliExecution> {
    lifecycle("status", None)
}

/// One admitted tool invocation, in the envelope both interfaces send.
///
/// The manual decoder below preserves the former `rename_all = "camelCase"`
/// wire contract without creating a second serde parser in the command layer.
struct Invocation {
    // The explicit wire keys below preserve `rename_all = "camelCase"`.
    name: String,
    arguments: Map<String, Value>,
    caller: CallerScope,
}

/// The scope the invoking adapter supplies for the call.
///
/// A conversation and a membership are present only when the adapter has them:
/// an inventory or a readiness probe carries neither. The domain owner decides
/// what an incomplete scope may do, and refuses it before any effect.
struct CallerScope {
    provider_id: String,
    conversation_id: Option<String>,
    membership_id: Option<String>,
    parent_dispatch_id: Option<String>,
}

fn optional_text(object: &Map<String, Value>, name: &str) -> Option<Option<String>> {
    match object.get(name) {
        None => Some(None),
        Some(Value::String(value)) => Some(Some(value.clone())),
        Some(_) => None,
    }
}

impl Invocation {
    fn decode(input: Value) -> Option<Self> {
        let object = input.as_object()?;
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "name" | "arguments" | "caller"))
        {
            return None;
        }
        Some(Self {
            name: object.get("name")?.as_str()?.to_owned(),
            arguments: object.get("arguments")?.as_object()?.clone(),
            caller: CallerScope::decode(object.get("caller")?)?,
        })
    }
}

impl CallerScope {
    fn decode(input: &Value) -> Option<Self> {
        let object = input.as_object()?;
        if object.keys().any(|key| {
            !matches!(
                key.as_str(),
                "providerId" | "conversationId" | "membershipId" | "parentDispatchId"
            )
        }) {
            return None;
        }
        Some(Self {
            provider_id: object.get("providerId")?.as_str()?.to_owned(),
            conversation_id: optional_text(object, "conversationId")?,
            membership_id: optional_text(object, "membershipId")?,
            parent_dispatch_id: optional_text(object, "parentDispatchId")?,
        })
    }

    fn claim(&self) -> ActorClaim {
        ActorClaim::Membership {
            provider_id: self.provider_id.clone(),
            conversation_id: self.conversation_id.clone(),
            membership_id: self.membership_id.clone(),
            parent_dispatch_id: self.parent_dispatch_id.clone(),
        }
    }
}

/// Run one invocation through the shared facade and publish its result.
fn execute_invocation(input: Value) -> Result<Value> {
    let invocation =
        Invocation::decode(input).ok_or_else(|| anyhow!("subagents_request_invalid"))?;
    let command = invocation
        .command()
        .ok_or_else(|| anyhow!("subagents_request_invalid"))?;
    Ok(
        match application_port::execute_as(&invocation.caller.claim(), &command) {
            CommandResolution::Resolved(outcome) => outcome.payload,
            CommandResolution::Failed(failure) => failure_envelope(&failure),
        },
    )
}

impl Invocation {
    /// The typed command one admitted tool name asks for.
    ///
    /// Arguments are checked against the domain owner's own tool schema first,
    /// so a call the owner would not admit is refused before a command is built
    /// out of it.
    fn command(&self) -> Option<ApplicationCommand> {
        if !validate_tool_arguments(&self.name, &self.arguments) {
            return None;
        }
        Some(match self.name.as_str() {
            "lico_subagents_list" => ApplicationCommand::Subagent(SubagentCommand::List),
            "lico_subagent_probe" => ApplicationCommand::Subagent(SubagentCommand::Probe {
                agent_id: self.text("agentId")?,
            }),
            "lico_subagent_delegate" => {
                ApplicationCommand::Subagent(SubagentCommand::Delegate(self.dispatch()?))
            }
            "lico_subagent_continue" => {
                ApplicationCommand::Subagent(SubagentCommand::Continue(self.dispatch()?))
            }
            "lico_subagent_cancel" => {
                ApplicationCommand::Subagent(SubagentCommand::Cancel(CancelRequest {
                    conversation_id: self.text("conversationId"),
                    membership_id: self.text("membershipId"),
                    agent_id: self.text("agent"),
                }))
            }
            "lico_assistant_profiles" => {
                ApplicationCommand::Assistant(AssistantCommand::Profiles {
                    conversation_id: self.text("conversationId")?,
                    filters: self.arguments.get("filters").cloned(),
                })
            }
            "lico_assistant_workflow_execute" => {
                ApplicationCommand::Assistant(AssistantCommand::WorkflowExecute {
                    conversation_id: self.text("conversationId")?,
                    membership_id: self.text("membershipId")?,
                    workflow: self.arguments.get("workflow").cloned()?,
                    bindings: self.arguments.get("bindings").cloned()?,
                    filters: self
                        .arguments
                        .get("filters")
                        .cloned()
                        .unwrap_or(Value::Null),
                    input: self.arguments.get("input").cloned(),
                    idempotency_key: self.text("idempotencyKey")?,
                    decision: self.decision()?,
                })
            }
            "lico_assistant_workflow_inspect" => {
                ApplicationCommand::Assistant(AssistantCommand::WorkflowInspect {
                    run_id: self.text("runId")?,
                })
            }
            "lico_assistant_workflow_cancel" => {
                ApplicationCommand::Assistant(AssistantCommand::WorkflowCancel {
                    run_id: self.text("runId")?,
                })
            }
            _ => return None,
        })
    }

    fn text(&self, name: &str) -> Option<String> {
        self.arguments
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    fn number(&self, name: &str) -> Option<u64> {
        self.arguments.get(name).and_then(Value::as_u64)
    }

    fn dispatch(&self) -> Option<DispatchRequest> {
        Some(DispatchRequest {
            conversation_id: self.text("conversationId"),
            membership_id: self.text("membershipId"),
            agent_id: self.text("agent"),
            prompt: self.text("prompt")?,
            model: self.text("model"),
            reasoning_effort: self.text("reasoningEffort"),
            working_directory: self.text("workingDirectory"),
            task_type: self.task_type(),
            timeout_ms: self.number("timeoutMs"),
            // The schema admits both spellings of the same request, and the
            // owner reads either, so one of them is published back.
            timeout_unbounded: ["timeoutUnbounded", "unboundedTimeout"]
                .iter()
                .any(|key| self.arguments.get(*key).and_then(Value::as_bool) == Some(true)),
            max_stdout_bytes: self.number("maxStdoutBytes"),
            max_stderr_bytes: self.number("maxStderrBytes"),
        })
    }

    fn task_type(&self) -> Option<TaskType> {
        match self.text("taskType")?.as_str() {
            "frontend" => Some(TaskType::Frontend),
            "backend" => Some(TaskType::Backend),
            "retrieval" => Some(TaskType::Retrieval),
            "text" => Some(TaskType::Text),
            _ => None,
        }
    }

    /// The callback decision an execute call answers a parked wait with.
    ///
    /// The schema admits the decision and its state fields independently, so an
    /// incomplete answer is refused here rather than turned into a decision the
    /// caller never made.
    fn decision(&self) -> Option<Option<CallbackDecision>> {
        let Some(decision) = self.text("decision") else {
            return Some(None);
        };
        Some(Some(match decision.as_str() {
            "terminate" => CallbackDecision::Terminate,
            "advance" | "return" => {
                let state_id = self.text("callbackStateId")?;
                let state_visit = self
                    .number("callbackStateVisit")
                    .filter(|visit| *visit >= 1)?;
                if decision == "advance" {
                    CallbackDecision::Advance {
                        state_id,
                        state_visit,
                    }
                } else {
                    CallbackDecision::Return {
                        state_id,
                        state_visit,
                    }
                }
            }
            _ => return None,
        }))
    }
}

/// This surface's own envelope for a refusal.
///
/// The vocabulary is the one the surface already publishes, and the recovery is
/// the MCP projection of the neutral action, so a caller sees the same
/// `code`/`stage`/`retryable`/`recovery` it has always seen for the same cause.
fn failure_envelope(failure: &ApplicationFailure) -> Value {
    json!({
        "schemaVersion": SUBAGENT_ERROR_SCHEMA,
        "reasonCode": &failure.code,
        "stage": &failure.stage,
        "retryable": failure.retryable,
        "recovery": failure.recovery.mcp_wire(),
        "isError": true,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{CliExecution, execute_cli};
    use super::*;
    use crate::domain::subagents::TOOL_NAMES;

    /// This surface decodes a request by tool name and the port names the work
    /// back to the owner by tool name. The two tables have to agree for every
    /// tool the owner publishes — not only for the ones a test happens to use.
    #[test]
    fn every_domain_tool_decodes_back_to_the_tool_it_was_asked_for() {
        for name in TOOL_NAMES {
            let invocation = decode(name, arguments_for(name));
            let command = invocation
                .command()
                .unwrap_or_else(|| panic!("{name} must decode"));
            let (translated, _) = match &command {
                ApplicationCommand::Subagent(command) => {
                    application_port::subagent_invocation(command)
                }
                ApplicationCommand::Assistant(command) => {
                    application_port::assistant_invocation(command)
                }
                ApplicationCommand::Conversation(_) | ApplicationCommand::Project(_) => {
                    panic!("{name} is not a tool")
                }
            };
            assert_eq!(translated, *name, "{name} decodes into another tool");
        }
        assert!(
            decode("lico_subagent_unknown", json!({}))
                .command()
                .is_none(),
            "a name the owner never published is not a command"
        );
    }

    #[test]
    fn a_call_that_breaks_the_owner_schema_is_refused() {
        let invocation = decode("lico_subagent_probe", json!({"agentId": "codex", "x": 1}));
        assert!(
            invocation.command().is_none(),
            "an argument the owner never admitted must not become a command"
        );
    }

    /// An adapter with no conversation for an inventory still calls it; the
    /// scope it does have is carried through unchanged.
    #[test]
    fn a_caller_scope_is_kept_exactly_as_the_adapter_supplied_it() {
        let bare = decode("lico_subagents_list", json!({}));
        assert_eq!(bare.caller.provider_id, "codex");
        assert_eq!(bare.caller.claim().conversation_id(), None);
        assert_eq!(bare.caller.claim().membership_id(), None);

        let scoped = decode_with(
            "lico_subagent_delegate",
            json!({"prompt": "work", "membershipId": "membership:worker"}),
            json!({
                "providerId": "codex",
                "conversationId": "conversation:one",
                "membershipId": "membership:worker",
                "parentDispatchId": "dispatch:parent",
            }),
        );
        let claim = scoped.caller.claim();
        assert_eq!(claim.conversation_id(), Some("conversation:one"));
        assert_eq!(claim.membership_id(), Some("membership:worker"));
        assert!(scoped.command().is_some());
    }

    /// A parked callback wait is answered with the state it answers; a terminal
    /// decision answers none. An incomplete answer is not a decision.
    #[test]
    fn only_a_complete_callback_decision_becomes_one() {
        let execute = |arguments| decode("lico_assistant_workflow_execute", arguments).command();
        let base = |decision: Value| {
            json!({
                "conversationId": "conversation:one",
                "membershipId": "membership:assistant",
                "workflow": {},
                "bindings": [],
                "idempotencyKey": "idem-1",
                "decision": decision,
            })
        };
        let mut advance = base(json!("advance"));
        advance["callbackStateId"] = json!("state:one");
        advance["callbackStateVisit"] = json!(2);
        assert!(execute(advance).is_some());

        let mut terminate = base(json!("terminate"));
        terminate["callbackStateId"] = json!("state:one");
        assert!(execute(terminate).is_some());

        assert!(
            execute(base(json!("advance"))).is_none(),
            "a parked decision without the state it answers is incomplete"
        );
        assert!(
            execute(base(json!("something-new"))).is_none(),
            "a decision the owner never published is not a decision"
        );
    }

    /// The invocation is this surface's own wire shape: a body with an extra
    /// field, or one missing its caller, is not a call the owner ever admitted.
    #[test]
    fn an_invocation_the_surface_never_published_is_refused_before_the_facade() {
        for input in [
            json!({"name": "lico_subagents_list", "arguments": {}}),
            json!({
                "name": "lico_subagents_list",
                "arguments": {},
                "caller": {"providerId": "codex", "authenticated": true},
            }),
            json!("not an object"),
        ] {
            assert_eq!(
                execute_invocation(input).unwrap_err().to_string(),
                "subagents_request_invalid"
            );
        }
    }

    /// The surface publishes the reason code it is given and the recovery the
    /// MCP vocabulary already uses, so a caller is told the same thing to do
    /// about the same cause.
    #[test]
    fn a_refusal_keeps_the_reason_code_and_projects_recovery() {
        for (failure, recovery) in [
            (
                ApplicationFailure::permanent("subagent_adapter_unavailable", "adapter/select"),
                "correct_request_and_retry",
            ),
            (
                ApplicationFailure::retryable(
                    "conversation_state_unavailable",
                    "conversation/store",
                ),
                "retry_after_recovery",
            ),
            (
                ApplicationFailure::uncertain(
                    "dispatch_reconciliation_required",
                    "dispatch/reconcile",
                ),
                "reconcile_before_retry",
            ),
        ] {
            let envelope = failure_envelope(&failure);
            assert_eq!(envelope["schemaVersion"], SUBAGENT_ERROR_SCHEMA);
            assert_eq!(envelope["reasonCode"], failure.code.as_str());
            assert_eq!(envelope["stage"], failure.stage.as_str());
            assert_eq!(envelope["retryable"], failure.retryable);
            assert_eq!(envelope["isError"], true);
            assert_eq!(envelope["recovery"], recovery);
        }
    }

    /// The whole surface, driven the way both interfaces drive it.
    ///
    /// The MCP process reaches delegated work by handing this exact argv to the
    /// CLI, so the argv is the cross-interface seam. A caller the mesh does not
    /// admit is refused by the facade's caller authority — before the domain
    /// owner is reached at all, and without a conversation host anywhere.
    #[test]
    fn the_admitted_cli_route_carries_an_invocation_to_the_facade() {
        let execution = execute_cli(vec![
            "subagents".to_owned(),
            "execute".to_owned(),
            "--stdin-json".to_owned(),
            json!({
                "name": "lico_subagent_delegate",
                "arguments": {"prompt": "work", "membershipId": "membership:worker"},
                "caller": {
                    "providerId": "not-a-provider",
                    "conversationId": "conversation:one",
                    "membershipId": "membership:worker",
                },
            })
            .to_string(),
        ])
        .expect("an admitted route answers with one JSON envelope");
        let CliExecution::Json(envelope) = execution else {
            panic!("subagents execute must print exactly one JSON envelope");
        };
        assert_eq!(envelope["schemaVersion"], SUBAGENT_ERROR_SCHEMA);
        assert_eq!(envelope["reasonCode"], "subagents_caller_invalid");
        assert_eq!(envelope["stage"], "caller/authorize");
        assert_eq!(envelope["retryable"], false);
        assert_eq!(envelope["isError"], true);
    }

    /// The catalog is the owner's published surface, not a second list kept by
    /// the interface.
    #[test]
    fn the_catalog_route_publishes_the_owners_own_tools() {
        let execution = execute_cli(vec!["subagents".to_owned(), "catalog".to_owned()])
            .expect("the catalog route is admitted without options");
        let CliExecution::Json(envelope) = execution else {
            panic!("subagents catalog must print exactly one JSON object");
        };
        let tools = envelope["tools"]
            .as_array()
            .expect("the catalog publishes a tool list");
        assert_eq!(tools.len(), TOOL_NAMES.len());
        assert!(
            envelope["callers"].is_array(),
            "the catalog publishes the admitted callers"
        );
    }

    /// One tool's minimum admissible arguments, as the owner's schema admits
    /// them.
    fn arguments_for(name: &str) -> Value {
        match name {
            "lico_subagent_probe" => json!({"agentId": "codex"}),
            "lico_subagent_delegate" | "lico_subagent_continue" => {
                json!({"prompt": "work", "membershipId": "membership:worker"})
            }
            "lico_subagent_cancel" => json!({"membershipId": "membership:worker"}),
            "lico_assistant_profiles" => json!({"conversationId": "conversation:one"}),
            "lico_assistant_workflow_execute" => json!({
                "conversationId": "conversation:one",
                "membershipId": "membership:assistant",
                "workflow": {},
                "bindings": [],
                "idempotencyKey": "idem-1",
            }),
            "lico_assistant_workflow_inspect" | "lico_assistant_workflow_cancel" => {
                json!({"runId": "run:one"})
            }
            _ => json!({}),
        }
    }

    fn decode(name: &str, arguments: Value) -> Invocation {
        decode_with(name, arguments, json!({"providerId": "codex"}))
    }

    fn decode_with(name: &str, arguments: Value, caller: Value) -> Invocation {
        Invocation::decode(json!({
            "name": name,
            "arguments": arguments,
            "caller": caller,
        }))
        .expect("this surface's own envelope decodes")
    }
}
