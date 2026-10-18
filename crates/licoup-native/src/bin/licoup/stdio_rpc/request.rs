use super::*;
use licoup_native::contracts::conversation_protocol::{
    ConversationCommand, ConversationProtocolMethod,
};
use licoup_native::ffi::generated::client_state::{
    ClientStateCollection, ClientStateFailure, ClientStateFailureCode,
};

#[path = "request/io_policy.rs"]
mod io_policy;
pub(crate) use io_policy::{rpc_command_reads_external_stdin, rpc_command_writes_external_stdout};

pub(crate) fn parse_stdio_rpc_request(
    bytes: &[u8],
) -> std::result::Result<StdioRpcRequest, StdioRpcRequestError> {
    // Method names, envelope fields, identifier bounds, and argument bounds
    // all come from the generated conversation protocol contract
    // (schemas/conversation_protocol/, generated into
    // licoup_native::contracts::conversation_protocol). The hand-written
    // method string table is gone: this file only maps generated method
    // variants onto the process-local dispatch shape.
    let command = ConversationCommand::decode(bytes).map_err(|error| StdioRpcRequestError {
        id: error.id,
        workflow_id: error.workflow_id,
        code: error.code,
    })?;
    let request_id = &command.id;
    let request_workflow_id = &command.workflow_id;
    let portable_data_dir = command.portable_data_dir;
    // The reply lane is schema-derived: the generated contract decides which
    // methods are in-flight control, so a cancel/steer/detach reply cannot be
    // classified as ordinary bulk by a second hand-written method table.
    let lane = reply_lane(&command.method);
    let method = match command.method {
        ConversationProtocolMethod::Execute => StdioRpcMethod::Execute {
            args: command
                .args
                .expect("generated execute commands always carry validated args"),
            portable_data_dir,
        },
        ConversationProtocolMethod::CatalogStatus
        | ConversationProtocolMethod::CatalogInvalidate
        | ConversationProtocolMethod::CatalogRefresh
        | ConversationProtocolMethod::CatalogReceipt
        | ConversationProtocolMethod::CatalogPurge
        | ConversationProtocolMethod::CatalogReconnect
        | ConversationProtocolMethod::CatalogList
        | ConversationProtocolMethod::CatalogObserve => StdioRpcMethod::Catalog {
            operation: command
                .method
                .as_str()
                .strip_prefix("catalog.")
                .unwrap_or_else(|| command.method.as_str())
                .to_string(),
            params: command.params,
            portable_data_dir,
        },
        ConversationProtocolMethod::StateGet => {
            let failure = ClientStateFailure::new(ClientStateFailureCode::InvalidCollection);
            let request = serde_json::from_value(command.params).map_err(|_| {
                invalid_request_id(request_id, request_workflow_id, failure.code.as_str())
            })?;
            StdioRpcMethod::StateGet {
                request,
                portable_data_dir,
            }
        }
        ConversationProtocolMethod::StateSet => {
            let collection_failure =
                ClientStateFailure::new(ClientStateFailureCode::InvalidCollection);
            command
                .params
                .get("collection")
                .cloned()
                .and_then(|value| serde_json::from_value::<ClientStateCollection>(value).ok())
                .ok_or_else(|| {
                    invalid_request_id(
                        request_id,
                        request_workflow_id,
                        collection_failure.code.as_str(),
                    )
                })?;
            let document_failure = ClientStateFailure::new(ClientStateFailureCode::InvalidDocument);
            let request = serde_json::from_value(command.params).map_err(|_| {
                invalid_request_id(
                    request_id,
                    request_workflow_id,
                    document_failure.code.as_str(),
                )
            })?;
            StdioRpcMethod::StateSet {
                request,
                portable_data_dir,
            }
        }
        ConversationProtocolMethod::AgentConversationOpen
        | ConversationProtocolMethod::AgentConversationSend
        | ConversationProtocolMethod::AgentConversationDispatch
        | ConversationProtocolMethod::AgentConversationCancel
        | ConversationProtocolMethod::AgentConversationHistory
        | ConversationProtocolMethod::AgentConversationCleanup
        | ConversationProtocolMethod::AgentConversationSteer
        | ConversationProtocolMethod::AgentConversationCapabilities
        | ConversationProtocolMethod::AgentConversationStream
        | ConversationProtocolMethod::AgentConversationActive
        | ConversationProtocolMethod::AgentConversationExecution
        | ConversationProtocolMethod::AgentConversationExecutionDetach
        | ConversationProtocolMethod::AgentConversationAttach
        | ConversationProtocolMethod::AgentConversationStop
        | ConversationProtocolMethod::AgentConversationForcePreview
        | ConversationProtocolMethod::AgentConversationForceConfirm => {
            StdioRpcMethod::Conversation {
                operation: command
                    .method
                    .as_str()
                    .strip_prefix("agent.conversation.")
                    .unwrap_or_else(|| command.method.as_str())
                    .to_string(),
                params: command.params,
                portable_data_dir,
            }
        }
        ConversationProtocolMethod::ClientConversationExecute => {
            StdioRpcMethod::ClientConversation {
                params: command.params,
                portable_data_dir,
            }
        }
        ConversationProtocolMethod::StrategyExecute => StdioRpcMethod::StrategyExecute {
            params: command.params,
            portable_data_dir,
        },
        ConversationProtocolMethod::SelectionMatrix
        | ConversationProtocolMethod::SelectionPolicyGet
        | ConversationProtocolMethod::SelectionPolicyAdopt
        | ConversationProtocolMethod::SelectionPolicySupersede
        | ConversationProtocolMethod::SelectionPolicyRevoke => {
            let operation = command
                .method
                .as_str()
                .strip_prefix("selection.")
                .unwrap_or_else(|| command.method.as_str());
            let request = SelectionRequest::parse(operation, command.params)
                .map_err(|code| invalid_request_id(request_id, request_workflow_id, code))?;
            StdioRpcMethod::Selection {
                request,
                portable_data_dir,
            }
        }
        // Structured private-stdin commands. The stdin JSON payload is carried
        // as structured params inside the RPC frame on the wire (never smuggled
        // through a CLI argument array); the process-local CLI admission is
        // reconstructed here inside the host boundary.
        ConversationProtocolMethod::TargetsScan => StdioRpcMethod::Execute {
            args: private_stdin_cli_args(
                &[
                    "targets",
                    "scan",
                    "--include-accessible-environments",
                    "true",
                ],
                None,
                command.params,
            ),
            portable_data_dir,
        },
        ConversationProtocolMethod::TargetsAdd => StdioRpcMethod::Execute {
            args: private_stdin_cli_args(
                &["targets", "add", "--target"],
                Some("target"),
                command.params,
            ),
            portable_data_dir,
        },
        ConversationProtocolMethod::GatewayCredentialsCreate => StdioRpcMethod::Execute {
            args: private_stdin_cli_args(
                &["llm-gateway", "credentials", "create"],
                None,
                command.params,
            ),
            portable_data_dir,
        },
        ConversationProtocolMethod::GatewayCredentialsUpdate => StdioRpcMethod::Execute {
            args: private_stdin_cli_args(
                &["llm-gateway", "credentials", "update"],
                Some("credentialId"),
                command.params,
            ),
            portable_data_dir,
        },
        ConversationProtocolMethod::Shutdown => StdioRpcMethod::Shutdown {
            params: command.params,
        },
        ConversationProtocolMethod::DataHomeRelocate => StdioRpcMethod::DataHomeRelocate {
            params: command.params,
        },
        ConversationProtocolMethod::DataHomeStatus => StdioRpcMethod::DataHomeStatus,
        ConversationProtocolMethod::DataHomeRecover => StdioRpcMethod::DataHomeRecover {
            params: command.params,
        },
        ConversationProtocolMethod::DataHomeCleanup => StdioRpcMethod::DataHomeCleanup {
            params: command.params,
        },
    };
    Ok(StdioRpcRequest {
        id: request_id.clone(),
        workflow_id: request_workflow_id.clone(),
        method,
        lane,
    })
}

/// The lane a reply to this request is written on.
///
/// The in-flight control methods are the ones the generated conversation
/// protocol marks `inFlightControl`: cancel, steer, detach and the stop/force
/// operations. Their replies must never queue behind bulk, because the bulk
/// they are meant to interrupt is exactly what is in front of them.
fn reply_lane(method: &ConversationProtocolMethod) -> StdioRpcFrameLane {
    use ConversationProtocolMethod as Method;
    match method {
        Method::AgentConversationCancel
        | Method::AgentConversationSteer
        | Method::AgentConversationStop
        | Method::AgentConversationForcePreview
        | Method::AgentConversationForceConfirm
        | Method::AgentConversationExecutionDetach => StdioRpcFrameLane::Control,
        _ => StdioRpcFrameLane::Ordered,
    }
}

fn invalid_request_id(
    id: &String,
    workflow_id: &String,
    code: &'static str,
) -> StdioRpcRequestError {
    StdioRpcRequestError {
        id: Some(id.clone()),
        workflow_id: Some(workflow_id.clone()),
        code,
    }
}

/// Rebuild the process-local CLI argv for a private-stdin structured command.
///
/// `argv` is the invariant leading CLI tokens, `positional` is the structured
/// param that must appear as the single positional-value token before the
/// `--stdin-json` JSON payload, and `params` becomes the trailing stdin JSON.
/// This keeps the JSON payload out of the CLI argument array across the RPC
/// wire while preserving the exact command shape the one-shot CLI path uses.
fn private_stdin_cli_args(argv: &[&str], positional: Option<&str>, params: Value) -> Vec<String> {
    let mut args: Vec<String> = argv.iter().map(|value| (*value).to_string()).collect();
    if let Some(positional_param) = positional {
        let value = params
            .get(positional_param)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or_default();
        args.push(value.to_string());
    }
    args.push("--stdin-json".to_string());
    args.push(serde_json::to_string(&params).expect("validated structured params serialize"));
    args
}

// The bin-level frame limits must stay congruent with the schema-derived
// conversation protocol contract. Keeping the compile-time comparison here
// makes a config drift a build error instead of a silent wire-format change.
const _: () = assert!(
    STDIO_RPC_MAX_ARGS
        == licoup_native::contracts::conversation_protocol::CONVERSATION_PROTOCOL_MAX_ARGS
);
const _: () = assert!(
    STDIO_RPC_MAX_ID_BYTES
        == licoup_native::contracts::conversation_protocol::CONVERSATION_PROTOCOL_MAX_ID_BYTES
);

/// Manual stop and force stop must keep routing to the owned conversation lane.
///
/// The generated contract declares the three wire names as conversation-lane
/// control methods; this file is the hand-written half that turns a decoded
/// method into a process-local operation. A wire name that stops being mapped
/// here stays invisible to the client while the schema still advertises it.
#[cfg(test)]
mod work_control_routing_tests {
    use super::*;
    use serde_json::json;

    fn route(method: ConversationProtocolMethod, params: Value) -> StdioRpcMethod {
        let frame = ConversationCommand::frame("request-1", "workflow-1", method, params);
        let bytes = serde_json::to_vec(&frame).expect("a protocol frame serializes");
        parse_stdio_rpc_request(&bytes)
            .expect("a well-formed control frame is admitted")
            .method
    }

    fn owned_operation(routed: StdioRpcMethod) -> (String, Value) {
        match routed {
            StdioRpcMethod::Conversation {
                operation, params, ..
            } => (operation, params),
            other => panic!("work control must use the conversation lane, got {other:?}"),
        }
    }

    #[test]
    fn work_control_methods_reach_their_native_operations() {
        for (method, expected) in [
            (ConversationProtocolMethod::AgentConversationStop, "stop"),
            (
                ConversationProtocolMethod::AgentConversationForcePreview,
                "force.preview",
            ),
            (
                ConversationProtocolMethod::AgentConversationForceConfirm,
                "force.confirm",
            ),
        ] {
            let (operation, _) = owned_operation(route(method, json!({})));
            assert_eq!(
                operation, expected,
                "{method:?} must reach its native owner"
            );
        }
    }

    #[test]
    fn work_control_params_survive_routing_unchanged() {
        let (operation, params) = owned_operation(route(
            ConversationProtocolMethod::AgentConversationStop,
            json!({"turnHandle": "turn-1", "conversationId": "c-1"}),
        ));
        assert_eq!(operation, "stop");
        assert_eq!(
            params,
            json!({"turnHandle": "turn-1", "conversationId": "c-1"})
        );

        // An omitted scope is how the client asks the host to name its own
        // candidate scopes; routing must not invent one.
        let (operation, params) = owned_operation(route(
            ConversationProtocolMethod::AgentConversationForcePreview,
            json!({}),
        ));
        assert_eq!(operation, "force.preview");
        assert_eq!(params, json!({}));

        let confirmed = json!({
            "scopeId": "scope-1",
            "confirmationToken": "token-1",
            "confirmed": true
        });
        let (operation, params) = owned_operation(route(
            ConversationProtocolMethod::AgentConversationForceConfirm,
            confirmed.clone(),
        ));
        assert_eq!(operation, "force.confirm");
        assert_eq!(params, confirmed);
    }
}

/// The selection surface must reach the owners that already hold the facts.
///
/// A method the generated contract advertises but this table does not map stays
/// invisible to the client, and a method mapped onto the wrong operation silently
/// answers a different question than the one that was asked. Both are checked
/// here, together with the parity guarantee the desktop projection depends on:
/// the matrix the client reads is the owner's document, unchanged.
#[cfg(test)]
mod selection_routing_tests {
    use super::*;
    use licoup_native::domain::client_conversation::{SelectionPolicyRevision, current_binding};
    use licoup_native::ffi::generated::client_error::ClientErrorCode;
    use serde_json::json;

    fn route(method: ConversationProtocolMethod, params: Value) -> StdioRpcMethod {
        let frame = ConversationCommand::frame("request-1", "workflow-1", method, params);
        let bytes = serde_json::to_vec(&frame).expect("a protocol frame serializes");
        parse_stdio_rpc_request(&bytes)
            .expect("a well-formed selection frame is admitted")
            .method
    }

    fn routed(method: ConversationProtocolMethod, params: Value) -> SelectionRequest {
        match route(method, params) {
            StdioRpcMethod::Selection { request, .. } => request,
            other => panic!("selection methods must use the selection lane, got {other:?}"),
        }
    }

    #[test]
    fn every_advertised_selection_method_reaches_its_operation() {
        let cases: Vec<(ConversationProtocolMethod, Value, &str)> = vec![
            (
                ConversationProtocolMethod::SelectionMatrix,
                json!({"agent": "codex"}),
                "matrix",
            ),
            (
                ConversationProtocolMethod::SelectionPolicyGet,
                json!({}),
                "policy.get",
            ),
            (
                ConversationProtocolMethod::SelectionPolicyAdopt,
                json!({"revision": {
                    "revisionId": "policy:r1",
                    "provenance": "feedback:outcome/policy:r1",
                    "preferences": {"preferredModel": "model-a"}
                }}),
                "policy.adopt",
            ),
            (
                ConversationProtocolMethod::SelectionPolicySupersede,
                json!({"revision": {
                    "revisionId": "policy:r2",
                    "parentRevisionId": "policy:r1",
                    "provenance": "feedback:outcome/policy:r2",
                    "preferences": {"preferredModel": "model-b"}
                }}),
                "policy.supersede",
            ),
            (
                ConversationProtocolMethod::SelectionPolicyRevoke,
                json!({"revisionId": "policy:r2"}),
                "policy.revoke",
            ),
        ];
        // Every advertised wire name is in the table under test: a new method
        // added to the schema without a mapping fails here instead of at the
        // client.
        assert_eq!(cases.len(), 5);
        for (method, params, expected) in cases {
            let reached = match routed(method, params) {
                SelectionRequest::Matrix { .. } => "matrix",
                SelectionRequest::PolicyGet => "policy.get",
                SelectionRequest::PolicyAdopt { .. } => "policy.adopt",
                SelectionRequest::PolicySupersede { .. } => "policy.supersede",
                SelectionRequest::PolicyRevoke { .. } => "policy.revoke",
            };
            assert_eq!(reached, expected, "{method:?} must reach its operation");
        }
    }

    #[test]
    fn the_revision_reaches_the_owner_exactly_as_the_client_stated_it() {
        let SelectionRequest::PolicySupersede { revision } = routed(
            ConversationProtocolMethod::SelectionPolicySupersede,
            json!({"revision": {
                "revisionId": "policy:r2",
                "parentRevisionId": "policy:r1",
                "provenance": "feedback:outcome/policy:r2",
                "preferences": {
                    "preferredModel": "model-b",
                    "preferredSkills": ["skill-b"]
                }
            }}),
        ) else {
            panic!("the supersede method must route to a supersede");
        };
        assert_eq!(
            revision,
            SelectionPolicyRevision {
                revision_id: "policy:r2".to_owned(),
                parent_revision_id: Some("policy:r1".to_owned()),
                provenance: "feedback:outcome/policy:r2".to_owned(),
                preferences:
                    licoup_native::domain::client_conversation::SelectionPolicyPreferences {
                        preferred_model: Some("model-b".to_owned()),
                        preferred_skills: vec!["skill-b".to_owned()],
                        ..Default::default()
                    },
            }
        );
    }

    #[test]
    fn the_probe_params_reach_the_catalogue_unchanged() {
        let SelectionRequest::Matrix { agent, params } = routed(
            ConversationProtocolMethod::SelectionMatrix,
            json!({"agent": "codex", "params": {"includeAccessibleEnvironments": true}}),
        ) else {
            panic!("the matrix method must route to a matrix");
        };
        assert_eq!(agent, "codex");
        assert_eq!(params, json!({"includeAccessibleEnvironments": true}));
    }

    #[test]
    fn a_matrix_the_catalogue_cannot_compose_is_an_error_and_not_an_empty_document() {
        // An Agent nothing declares cannot be observed. The client must be told
        // the facts could not be read, because an empty matrix would render as
        // "this Agent offers nothing" — a claim this host never made.
        let error = SelectionRequest::Matrix {
            agent: "not-a-declared-agent".to_owned(),
            params: json!({}),
        }
        .dispatch()
        .expect_err("an unobservable target is not an empty matrix");
        assert_eq!(error.code, ClientErrorCode::SelectionMatrixUnavailable);
    }

    #[test]
    fn a_refused_request_is_refused_before_any_owner_is_called() {
        for (method, params) in [
            (ConversationProtocolMethod::SelectionMatrix, json!({})),
            (
                ConversationProtocolMethod::SelectionMatrix,
                json!({"agent": "codex", "unknown": 1}),
            ),
            (
                ConversationProtocolMethod::SelectionPolicyGet,
                json!({"agent": "codex"}),
            ),
            (
                ConversationProtocolMethod::SelectionPolicyAdopt,
                json!({"revision": {"revisionId": ""}}),
            ),
            (
                ConversationProtocolMethod::SelectionPolicyAdopt,
                json!({"revision": {"revisionId": "policy:r1", "provenance": "p", "extra": 1}}),
            ),
            (
                ConversationProtocolMethod::SelectionPolicySupersede,
                json!({"parentRevisionId": "policy:r1"}),
            ),
            (
                ConversationProtocolMethod::SelectionPolicyRevoke,
                json!({"revisionId": "   "}),
            ),
        ] {
            let frame = ConversationCommand::frame("request-1", "workflow-1", method, params);
            let bytes = serde_json::to_vec(&frame).expect("a protocol frame serializes");
            let error = parse_stdio_rpc_request(&bytes)
                .err()
                .unwrap_or_else(|| panic!("{method:?} must refuse a malformed request"));
            assert_eq!(error.code, "invalid_params", "{method:?}");
        }
    }

    /// The read returns the binding the admission boundary captures, with the
    /// owner's own name for the revision in force rather than a recomputed one.
    #[test]
    fn the_policy_read_reports_the_owner_binding() {
        let SelectionRequest::PolicyGet =
            routed(ConversationProtocolMethod::SelectionPolicyGet, json!({}))
        else {
            panic!("the policy read must route to the policy read");
        };
        let binding = current_binding();
        let document = policy_document(&binding);
        assert_eq!(
            document["revisionName"],
            json!(binding.revision_name()),
            "the reported revision name is the owner's, not a recomputed one"
        );
        assert_eq!(
            document["revisionId"],
            binding
                .revision_id
                .clone()
                .map_or(Value::Null, Value::String)
        );
    }
}
