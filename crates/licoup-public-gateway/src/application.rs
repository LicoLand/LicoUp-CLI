use crate::{McpApplication, McpApplicationError, McpServerDefinition, McpToolCallContext};
use anyhow::{Result, anyhow};
use serde_json::{Map, Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};

pub const PROTOCOL_REVISION: &str = "2025-06-18";
pub const SERVER_NAME: &str = "lico-up-subagents";
pub const SERVER_VERSION: &str = "0.14.0";
pub const MAX_MCP_FRAME_BYTES: usize = 64 * 1024;
pub const REMOTE_TOOL_NAMES: &[&str] = &[
    "lico_subagents_list",
    "lico_subagent_probe",
    "lico_subagent_delegate",
    "lico_subagent_continue",
    "lico_subagent_cancel",
];
pub const TOOL_NAMES: &[&str] = REMOTE_TOOL_NAMES;

pub fn server_definition() -> McpServerDefinition {
    McpServerDefinition {
        protocol_revision: PROTOCOL_REVISION,
        compatible_protocol_revisions: &["2025-11-25"],
        server_name: SERVER_NAME,
        server_version: SERVER_VERSION,
        max_message_bytes: MAX_MCP_FRAME_BYTES,
    }
}

#[derive(Clone, Debug)]
pub struct CallerContext {
    pub provider_id: String,
    pub conversation_id: Option<String>,
    pub membership_id: Option<String>,
    pub parent_dispatch_id: Option<String>,
    pub authenticated: bool,
}

/// A reusable native CLI session. A bounded transport pool lets independent
/// admissions progress while PersistentTurn execution stays in the host.
struct CliClient {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    sequence: u64,
}
impl CliClient {
    fn start() -> Result<Self> {
        let binary = std::env::var_os("LICOUP_CLI_BINARY")
            .ok_or_else(|| anyhow!("mcp_cli_binary_required"))?;
        let mut child = Command::new(binary)
            .args(["rpc", "stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| anyhow!("mcp_cli_unavailable"))?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("mcp_cli_unavailable"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("mcp_cli_unavailable"))?;
        Ok(Self {
            child,
            input: Some(input),
            output: BufReader::new(output),
            sequence: 0,
        })
    }

    fn execute(&mut self, args: Vec<String>) -> Result<Value> {
        self.sequence += 1;
        let id = self.sequence.to_string();
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| anyhow!("mcp_cli_unavailable"))?;
        serde_json::to_writer(
            &mut *input,
            &json!({
                "protocol":"licoup.stdio.v1", "id":id,
                "workflowId":"mcp-service", "method":"execute", "args":args
            }),
        )?;
        input.write_all(b"\n")?;
        input.flush()?;
        // No task deadline. EOF is a transport failure, never an implicit cancel.
        let mut bytes = Vec::new();
        let count = (&mut self.output)
            .take(16 * 1024 * 1024 + 1)
            .read_until(b'\n', &mut bytes)?;
        if count == 0 || bytes.last() != Some(&b'\n') || bytes.len() > 16 * 1024 * 1024 {
            return Err(anyhow!("mcp_cli_response_invalid"));
        }
        let frame: Value = serde_json::from_slice(&bytes)?;
        if frame["protocol"] != "licoup.stdio.v1"
            || frame["id"] != id
            || frame["workflowId"] != "mcp-service"
        {
            return Err(anyhow!("mcp_cli_response_invalid"));
        }
        if frame["ok"] != true {
            return Err(anyhow!("mcp_cli_request_failed"));
        }
        frame
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow!("mcp_cli_response_invalid"))
    }
}
impl Drop for CliClient {
    fn drop(&mut self) {
        drop(self.input.take());
        // Closing this observer never kills the separate PersistentTurn host.
        let _ = self.child.wait();
    }
}

#[derive(Clone)]
pub struct SubagentMcpApplication {
    clients: Arc<Mutex<Vec<CliClient>>>,
    control: Arc<Mutex<Vec<CliClient>>>,
    catalog: Arc<Vec<Value>>,
    callers: Arc<Vec<String>>,
}
pub fn production_application() -> Result<SubagentMcpApplication> {
    let mut client = CliClient::start()?;
    let value = client.execute(vec!["subagents".into(), "catalog".into()])?;
    let catalog = admitted_catalog(&value)?;
    let callers = value
        .get("callers")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("mcp_cli_catalog_invalid"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("mcp_cli_catalog_invalid"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(SubagentMcpApplication {
        clients: Arc::new(Mutex::new(vec![client])),
        control: Arc::new(Mutex::new(Vec::new())),
        catalog: Arc::new(catalog),
        callers: Arc::new(callers),
    })
}

fn admitted_catalog(value: &Value) -> Result<Vec<Value>> {
    let tools = value
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("mcp_cli_catalog_invalid"))?;
    REMOTE_TOOL_NAMES
        .iter()
        .map(|name| {
            let mut matches = tools.iter().filter(|tool| tool["name"] == *name);
            let tool = matches
                .next()
                .filter(|tool| tool["inputSchema"].is_object())
                .ok_or_else(|| anyhow!("mcp_cli_catalog_invalid"))?;
            if matches.next().is_some() {
                return Err(anyhow!("mcp_cli_catalog_invalid"));
            }
            Ok(tool.clone())
        })
        .collect()
}
impl SubagentMcpApplication {
    pub fn caller_providers(&self) -> Vec<String> {
        self.callers.as_ref().clone()
    }
}
impl McpApplication for SubagentMcpApplication {
    type CallerContext = CallerContext;
    fn tool_catalog(&self) -> Vec<Value> {
        self.catalog.as_ref().clone()
    }
    fn validate_tool_arguments(&self, name: &str, arguments: &Map<String, Value>) -> bool {
        REMOTE_TOOL_NAMES.contains(&name)
            && self
                .catalog
                .iter()
                .find(|tool| tool["name"] == name)
                .is_some_and(|tool| valid_arguments(&tool["inputSchema"], arguments))
    }
    fn call_tool(
        &self,
        context: McpToolCallContext<'_, CallerContext>,
        name: &str,
        arguments: &Map<String, Value>,
    ) -> Result<Value, McpApplicationError> {
        if !context.caller.authenticated || !REMOTE_TOOL_NAMES.contains(&name) {
            return Err(McpApplicationError::permanent(
                "remote_operation_not_allowed",
                "admission",
            ));
        }
        let request = cli_invocation(name, arguments, context.caller);
        // A slow inventory/admission call cannot occupy the cancellation
        // session. HTTP admission bounds the normal pool to eight sessions and
        // reserves one separate control slot. No domain scheduling lives here.
        let pool = if name == "lico_subagent_cancel" {
            &self.control
        } else {
            &self.clients
        };
        let mut client = pool
            .lock()
            .map_err(|_| McpApplicationError::retryable("native_unavailable", "cli"))?
            .pop()
            .map(Ok)
            .unwrap_or_else(CliClient::start)
            .map_err(|_| McpApplicationError::retryable("native_unavailable", "cli"))?;
        let response = client.execute(vec![
            "subagents".into(),
            "execute".into(),
            "--stdin-json".into(),
            request.to_string(),
        ]);
        if response.is_ok() {
            pool.lock()
                .map_err(|_| McpApplicationError::retryable("native_unavailable", "cli"))?
                .push(client);
        }
        let response =
            response.map_err(|_| McpApplicationError::retryable("native_unavailable", "cli"))?;
        // Native errors are already public/redacted. Preserve their exact
        // payload without interpreting or duplicating domain policy here.
        Ok(response)
    }
}

/// Build the native CLI invocation without inventing caller scope.
///
/// The CLI distinguishes an omitted optional binding from an explicit JSON
/// `null`: the former is an unscoped inventory/probe call, while the latter is
/// malformed context. Keep that distinction across the independent-process
/// boundary and forward every declared value unchanged.
fn cli_invocation(name: &str, arguments: &Map<String, Value>, caller: &CallerContext) -> Value {
    let mut scope = Map::new();
    scope.insert("providerId".to_owned(), json!(caller.provider_id));
    insert_optional(
        &mut scope,
        "conversationId",
        caller.conversation_id.as_ref(),
    );
    insert_optional(&mut scope, "membershipId", caller.membership_id.as_ref());
    insert_optional(
        &mut scope,
        "parentDispatchId",
        caller.parent_dispatch_id.as_ref(),
    );
    json!({"name": name, "arguments": arguments, "caller": scope})
}

fn insert_optional(scope: &mut Map<String, Value>, key: &str, value: Option<&String>) {
    if let Some(value) = value {
        scope.insert(key.to_owned(), json!(value));
    }
}

// Validate the closed scalar schemas obtained from the CLI, without carrying a
// second domain schema. Native admission repeats its authoritative validation.
fn valid_arguments(schema: &Value, arguments: &Map<String, Value>) -> bool {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return false;
    };
    if arguments.keys().any(|key| !properties.contains_key(key)) {
        return false;
    }
    if schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|name| !arguments.contains_key(name))
    {
        return false;
    }
    arguments.iter().all(|(key, value)| {
        let property = &properties[key];
        if property["enum"]
            .as_array()
            .is_some_and(|choices| !choices.contains(value))
        {
            return false;
        }
        match property["type"].as_str() {
            Some("string") => value.as_str().is_some_and(|text| {
                !text.trim().is_empty()
                    && !text.contains('\0')
                    && text.len() as u64 >= property["minLength"].as_u64().unwrap_or(0)
                    && text.len() as u64 <= property["maxLength"].as_u64().unwrap_or(u64::MAX)
            }),
            Some("integer") => value.as_u64().is_some_and(|number| {
                number >= property["minimum"].as_u64().unwrap_or(0)
                    && number <= property["maximum"].as_u64().unwrap_or(u64::MAX)
            }),
            Some("boolean") => value.is_boolean(),
            Some("object") => value.is_object(),
            Some("array") => value.as_array().is_some_and(|items| {
                items.len() as u64 <= property["maxItems"].as_u64().unwrap_or(u64::MAX)
            }),
            _ => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_invocation_omits_undeclared_scope_and_preserves_declared_context() {
        let unscoped = CallerContext {
            provider_id: "codex".to_owned(),
            conversation_id: None,
            membership_id: None,
            parent_dispatch_id: None,
            authenticated: true,
        };
        let request = cli_invocation("lico_subagents_list", &Map::new(), &unscoped);
        assert_eq!(
            request,
            json!({
                "name": "lico_subagents_list",
                "arguments": {},
                "caller": {"providerId": "codex"},
            })
        );

        let scoped = CallerContext {
            provider_id: "cursor".to_owned(),
            conversation_id: Some("conversation:one".to_owned()),
            membership_id: Some("membership:worker".to_owned()),
            parent_dispatch_id: Some("dispatch:parent".to_owned()),
            authenticated: true,
        };
        let request = cli_invocation(
            "lico_subagent_delegate",
            &Map::from_iter([("prompt".to_owned(), json!("ordinary text"))]),
            &scoped,
        );
        assert_eq!(
            request["caller"],
            json!({
                "providerId": "cursor",
                "conversationId": "conversation:one",
                "membershipId": "membership:worker",
                "parentDispatchId": "dispatch:parent",
            })
        );
        assert_eq!(request["arguments"]["prompt"], "ordinary text");
        assert!(request["caller"].get("authenticated").is_none());
    }

    #[test]
    fn only_complete_subagents_operations_are_remotely_admitted() {
        let tools = REMOTE_TOOL_NAMES
            .iter()
            .chain(["lico_assistant_profiles", "conversation.execute"].iter())
            .map(|name| json!({"name":name,"inputSchema":{"type":"object"}}))
            .collect::<Vec<_>>();
        let catalog = admitted_catalog(&json!({"tools":tools})).unwrap();
        assert_eq!(catalog.len(), 5);
        assert_eq!(
            catalog
                .iter()
                .map(|v| v["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            REMOTE_TOOL_NAMES
        );
        assert!(admitted_catalog(&json!({"tools":[]})).is_err());
    }
}
