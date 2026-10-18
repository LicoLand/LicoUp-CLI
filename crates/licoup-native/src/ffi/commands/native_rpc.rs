//! Native CLI admission and presentation for the persistent RPC surface.

use super::{AdmittedCommand, CliExecution};
use crate::contracts::conversation_protocol::ConversationProtocolMethod;
use crate::platform::conversation_host_client::{self, HostResponse};
use anyhow::{Result, anyhow};
use serde_json::Value;
use std::io::{self, Read, Write};

pub use crate::platform::conversation_host_client::HostOutput;

/// Validate before the executable starts a host or touches client state.
pub fn request_for_command(command: &AdmittedCommand) -> Result<Option<(Value, HostOutput)>> {
    let (method, output) = match command.path() {
        ["rpc", "call"] => (
            ConversationProtocolMethod::from_wire(command.required_text("method"))
                .ok_or_else(|| anyhow!("invalid_method"))?,
            HostOutput::Frames,
        ),
        ["conversation", "execute"] => (
            ConversationProtocolMethod::ClientConversationExecute,
            HostOutput::Result,
        ),
        ["strategy", "execute"] => (
            ConversationProtocolMethod::StrategyExecute,
            HostOutput::Result,
        ),
        _ => return Ok(None),
    };
    let params = command
        .option_json("stdin-json")
        .ok_or_else(|| anyhow!("invalid_params"))?;
    Ok(Some((
        conversation_host_client::request_for_method(method, params)?,
        output,
    )))
}

pub(super) fn handle_rpc_call(command: AdmittedCommand) -> Result<CliExecution> {
    let method = ConversationProtocolMethod::from_wire(command.required_text("method"))
        .ok_or_else(|| anyhow!("invalid_method"))?;
    let params = command
        .option_json("stdin-json")
        .ok_or_else(|| anyhow!("invalid_params"))?;
    let request = conversation_host_client::request_for_method(method, params)?;
    let stream = crate::platform::conversation_host_transport::connect_existing()
        .map_err(|_| anyhow!("persistent_conversation_transport_required"))?;
    execute_host_call(stream, request, io::stdout().lock(), HostOutput::Frames)
}

/// Project the platform's neutral response into the CLI presentation result.
pub fn execute_host_call(
    stream: impl Read + Write,
    request: Value,
    output: impl Write,
    mode: HostOutput,
) -> Result<CliExecution> {
    Ok(
        match conversation_host_client::execute_host_call(stream, request, output, mode)? {
            HostResponse::Json(value) => CliExecution::Json(value),
            HostResponse::Streamed => CliExecution::Streamed,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::conversation_protocol::{
        CONVERSATION_PROTOCOL_METHODS, ConversationCommand,
    };
    use crate::ffi::commands::admit_cli_command;

    fn admitted(method: &str, params: &str) -> AdmittedCommand {
        admit_cli_command(
            ["rpc", "call", method, "--stdin-json", params]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn every_generated_native_method_has_a_cli_call() {
        for method in CONVERSATION_PROTOCOL_METHODS {
            let params = if method == "execute" {
                r#"{"args":["commands"]}"#
            } else {
                "{}"
            };
            let (frame, mode) = request_for_command(&admitted(method, params))
                .unwrap()
                .unwrap();
            assert_eq!(frame["method"], method);
            assert_eq!(mode, HostOutput::Frames);
            ConversationCommand::decode(&serde_json::to_vec(&frame).unwrap()).unwrap();
        }
        assert!(request_for_command(&admitted("unknown-private-method", "{}")).is_err());
        assert!(request_for_command(&admitted("execute", "{}")).is_err());
        assert!(request_for_command(&admitted("catalog.list", "[]")).is_err());
    }
}
