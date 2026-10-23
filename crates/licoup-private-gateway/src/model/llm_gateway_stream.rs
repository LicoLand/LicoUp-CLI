//! Incremental SSE protocol conversion for the local LLM gateway.

use crate::model::llm_gateway::{
    ClientProtocol, GatewayError, MAX_GATEWAY_BODY_BYTES, PreparedGatewayRequest, UpstreamProtocol,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub struct GatewayStreamTransformer {
    mode: StreamMode,
    pending: Vec<u8>,
    received: usize,
    finished: bool,
}

pub struct StreamedChatResponse {
    pub(crate) response_id: String,
    pub(crate) assistant: Value,
}

enum StreamMode {
    Passthrough,
    Responses(ChatStreamState),
    Anthropic(ChatStreamState),
}

struct ChatStreamState {
    model: String,
    id: Option<String>,
    sequence: u64,
    next_output_index: usize,
    reasoning: Option<OutputState>,
    text: Option<OutputState>,
    tools: BTreeMap<usize, ToolState>,
    finish_reason: Option<String>,
    usage: Option<Value>,
    started: bool,
    history_response: Option<StreamedChatResponse>,
}

struct OutputState {
    output_index: usize,
    value: String,
}

#[derive(Default)]
struct ToolState {
    output_index: Option<usize>,
    id: String,
    name: String,
    arguments: String,
    emitted_arguments: usize,
}

impl GatewayStreamTransformer {
    pub fn new(request: &PreparedGatewayRequest) -> Result<Self, GatewayError> {
        let state = || ChatStreamState {
            model: request.requested_model.clone(),
            id: None,
            sequence: 1,
            next_output_index: 0,
            reasoning: None,
            text: None,
            tools: BTreeMap::new(),
            finish_reason: None,
            usage: None,
            started: false,
            history_response: None,
        };
        let mode = match (request.client_protocol, request.upstream_protocol) {
            (ClientProtocol::AnthropicMessages, UpstreamProtocol::AnthropicMessages)
            | (ClientProtocol::OpenAiChatCompletions, UpstreamProtocol::OpenAiChatCompletions)
            | (ClientProtocol::OpenAiResponses, UpstreamProtocol::OpenAiResponses) => {
                StreamMode::Passthrough
            }
            (ClientProtocol::OpenAiResponses, UpstreamProtocol::OpenAiChatCompletions) => {
                StreamMode::Responses(state())
            }
            (ClientProtocol::AnthropicMessages, UpstreamProtocol::OpenAiChatCompletions) => {
                StreamMode::Anthropic(state())
            }
            _ => return Err(GatewayError::UnsupportedConversion),
        };
        Ok(Self {
            mode,
            pending: Vec::new(),
            received: 0,
            finished: false,
        })
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<u8>, GatewayError> {
        self.received = self
            .received
            .checked_add(bytes.len())
            .filter(|total| *total <= MAX_GATEWAY_BODY_BYTES)
            .ok_or(GatewayError::RequestTooLarge)?;
        if matches!(self.mode, StreamMode::Passthrough) {
            return Ok(bytes.to_vec());
        }
        self.pending.extend_from_slice(bytes);
        let mut output = Vec::new();
        while let Some((event_end, delimiter_len)) = next_event_boundary(&self.pending) {
            let event = self.pending.drain(..event_end).collect::<Vec<_>>();
            self.pending.drain(..delimiter_len);
            let Some(data) = event_data(&event)? else {
                continue;
            };
            if data == "[DONE]" {
                output.extend(self.finalize()?);
                self.finished = true;
                continue;
            }
            let chunk: Value =
                serde_json::from_str(&data).map_err(|_| GatewayError::InvalidUpstreamResponse)?;
            match &mut self.mode {
                StreamMode::Responses(state) => output.extend(state.responses_chunk(&chunk)?),
                StreamMode::Anthropic(state) => output.extend(state.anthropic_chunk(&chunk)?),
                StreamMode::Passthrough => unreachable!(),
            }
        }
        Ok(output)
    }

    pub fn finish(&mut self) -> Result<Vec<u8>, GatewayError> {
        if matches!(self.mode, StreamMode::Passthrough) || self.finished {
            return Ok(Vec::new());
        }
        if !self.pending.iter().all(u8::is_ascii_whitespace) {
            return Err(GatewayError::InvalidUpstreamResponse);
        }
        self.finalize()
    }

    pub fn take_chat_history_response(&mut self) -> Option<StreamedChatResponse> {
        match &mut self.mode {
            StreamMode::Responses(state) => state.history_response.take(),
            StreamMode::Anthropic(_) | StreamMode::Passthrough => None,
        }
    }

    fn finalize(&mut self) -> Result<Vec<u8>, GatewayError> {
        match &mut self.mode {
            StreamMode::Responses(state) => state.finish_responses(),
            StreamMode::Anthropic(state) => state.finish_anthropic(),
            StreamMode::Passthrough => Ok(Vec::new()),
        }
    }
}

impl ChatStreamState {
    fn observe(&mut self, chunk: &Value) {
        if self.id.is_none() {
            self.id = chunk.get("id").and_then(Value::as_str).map(str::to_owned);
        }
        if let Some(reason) = chunk
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)
        {
            self.finish_reason = Some(reason.to_owned());
        }
        if let Some(usage) = chunk.get("usage") {
            self.usage = Some(usage.clone());
        }
    }

    fn response_id(&self) -> String {
        self.id
            .as_deref()
            .map(|id| format!("resp_{id}"))
            .unwrap_or_else(|| "resp_gateway".to_owned())
    }

    fn message_id(&self) -> &str {
        self.id.as_deref().unwrap_or("gateway-message")
    }

    fn responses_chunk(&mut self, chunk: &Value) -> Result<Vec<u8>, GatewayError> {
        self.observe(chunk);
        let mut output = Vec::new();
        if !self.started {
            self.started = true;
            let id = self.response_id();
            self.emit_response(
                &mut output,
                json!({"type":"response.created","response":response_shell(&id, &self.model, "in_progress", Vec::<Value>::new(), Value::Null)}),
            );
        }
        let Some(delta) = chunk.pointer("/choices/0/delta") else {
            return Ok(output);
        };
        if let Some(part) = delta
            .get("reasoning_content")
            .or_else(|| delta.get("reasoning"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            if self.reasoning.is_none() {
                let output_index = self.allocate_output();
                let id = self.response_id();
                self.emit_response(&mut output, json!({"type":"response.output_item.added","output_index":output_index,"item":{"id":format!("rs_{id}"),"type":"reasoning","status":"in_progress","summary":[]}}));
                self.emit_response(&mut output, json!({"type":"response.reasoning_summary_part.added","output_index":output_index,"summary_index":0,"part":{"type":"summary_text","text":""}}));
                self.reasoning = Some(OutputState {
                    output_index,
                    value: String::new(),
                });
            }
            let output_index = self.reasoning.as_ref().unwrap().output_index;
            self.reasoning.as_mut().unwrap().value.push_str(part);
            self.emit_response(&mut output, json!({"type":"response.reasoning_summary_text.delta","output_index":output_index,"summary_index":0,"delta":part}));
        }
        if let Some(part) = delta
            .get("content")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            if self.text.is_none() {
                let output_index = self.allocate_output();
                let id = self.response_id();
                self.emit_response(&mut output, json!({"type":"response.output_item.added","output_index":output_index,"item":{"id":format!("msg_{id}"),"type":"message","status":"in_progress","role":"assistant","content":[]}}));
                self.emit_response(&mut output, json!({"type":"response.content_part.added","output_index":output_index,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}));
                self.text = Some(OutputState {
                    output_index,
                    value: String::new(),
                });
            }
            let output_index = self.text.as_ref().unwrap().output_index;
            self.text.as_mut().unwrap().value.push_str(part);
            self.emit_response(&mut output, json!({"type":"response.output_text.delta","output_index":output_index,"content_index":0,"delta":part}));
        }
        self.update_response_tools(delta, &mut output)?;
        Ok(output)
    }

    fn update_response_tools(
        &mut self,
        delta: &Value,
        output: &mut Vec<u8>,
    ) -> Result<(), GatewayError> {
        for call in delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let mut tool = self.tools.remove(&index).unwrap_or_default();
            append_tool_fields(&mut tool, call);
            if tool.output_index.is_none() && !tool.id.is_empty() && !tool.name.is_empty() {
                let output_index = self.allocate_output();
                tool.output_index = Some(output_index);
                self.emit_response(output, json!({"type":"response.output_item.added","output_index":output_index,"item":{"id":tool.id,"type":"function_call","status":"in_progress","call_id":tool.id,"name":tool.name,"arguments":""}}));
            }
            if let Some(output_index) = tool.output_index {
                let delta = tool
                    .arguments
                    .get(tool.emitted_arguments..)
                    .ok_or(GatewayError::InvalidUpstreamResponse)?
                    .to_owned();
                if !delta.is_empty() {
                    self.emit_response(output, json!({"type":"response.function_call_arguments.delta","output_index":output_index,"delta":delta}));
                    tool.emitted_arguments = tool.arguments.len();
                }
            }
            self.tools.insert(index, tool);
        }
        Ok(())
    }

    fn anthropic_chunk(&mut self, chunk: &Value) -> Result<Vec<u8>, GatewayError> {
        self.observe(chunk);
        let mut output = Vec::new();
        if !self.started {
            self.started = true;
            output.extend(named_event("message_start", json!({"type":"message_start","message":{"id":self.message_id(),"type":"message","role":"assistant","model":self.model,"content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}}))?);
        }
        let Some(delta) = chunk.pointer("/choices/0/delta") else {
            return Ok(output);
        };
        if let Some(text) = delta
            .get("content")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            if self.text.is_none() {
                let index = self.allocate_output();
                output.extend(named_event("content_block_start", json!({"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}}))?);
                self.text = Some(OutputState {
                    output_index: index,
                    value: String::new(),
                });
            }
            let index = self.text.as_ref().unwrap().output_index;
            self.text.as_mut().unwrap().value.push_str(text);
            output.extend(named_event("content_block_delta", json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":text}}))?);
        }
        for call in delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let mut tool = self.tools.remove(&index).unwrap_or_default();
            append_tool_fields(&mut tool, call);
            if tool.output_index.is_none() && !tool.id.is_empty() && !tool.name.is_empty() {
                let output_index = self.allocate_output();
                tool.output_index = Some(output_index);
                output.extend(named_event("content_block_start", json!({"type":"content_block_start","index":output_index,"content_block":{"type":"tool_use","id":tool.id,"name":tool.name,"input":{}}}))?);
            }
            if let Some(output_index) = tool.output_index {
                let delta = tool
                    .arguments
                    .get(tool.emitted_arguments..)
                    .ok_or(GatewayError::InvalidUpstreamResponse)?
                    .to_owned();
                if !delta.is_empty() {
                    output.extend(named_event("content_block_delta", json!({"type":"content_block_delta","index":output_index,"delta":{"type":"input_json_delta","partial_json":delta}}))?);
                    tool.emitted_arguments = tool.arguments.len();
                }
            }
            self.tools.insert(index, tool);
        }
        Ok(output)
    }

    fn finish_responses(&mut self) -> Result<Vec<u8>, GatewayError> {
        self.history_response = Some(self.project_history_response());
        let mut output = Vec::new();
        let mut completed = Vec::new();
        let id = self.response_id();
        if let Some(reasoning) = self.reasoning.take() {
            self.emit_response(&mut output, json!({"type":"response.reasoning_summary_text.done","output_index":reasoning.output_index,"summary_index":0,"text":reasoning.value}));
            let item = json!({"id":format!("rs_{id}"),"type":"reasoning","status":"completed","summary":[{"type":"summary_text","text":reasoning.value}]});
            self.emit_response(&mut output, json!({"type":"response.output_item.done","output_index":reasoning.output_index,"item":item}));
            completed.push((reasoning.output_index, item));
        }
        if let Some(text) = self.text.take() {
            self.emit_response(&mut output, json!({"type":"response.output_text.done","output_index":text.output_index,"content_index":0,"text":text.value}));
            let part = json!({"type":"output_text","text":text.value,"annotations":[]});
            self.emit_response(&mut output, json!({"type":"response.content_part.done","output_index":text.output_index,"content_index":0,"part":part}));
            let item = json!({"id":format!("msg_{id}"),"type":"message","status":"completed","role":"assistant","content":[part]});
            self.emit_response(&mut output, json!({"type":"response.output_item.done","output_index":text.output_index,"item":item}));
            completed.push((text.output_index, item));
        }
        let tools = std::mem::take(&mut self.tools);
        for (_, tool) in tools {
            let output_index = tool
                .output_index
                .ok_or(GatewayError::InvalidUpstreamResponse)?;
            self.emit_response(&mut output, json!({"type":"response.function_call_arguments.done","output_index":output_index,"arguments":tool.arguments}));
            let item = json!({"id":tool.id,"type":"function_call","status":"completed","call_id":tool.id,"name":tool.name,"arguments":tool.arguments});
            self.emit_response(
                &mut output,
                json!({"type":"response.output_item.done","output_index":output_index,"item":item}),
            );
            completed.push((output_index, item));
        }
        completed.sort_by_key(|(index, _)| *index);
        let completed = completed.into_iter().map(|(_, item)| item).collect();
        let usage = responses_usage(self.usage.as_ref());
        let (status, incomplete_details) = responses_completion(self.finish_reason.as_deref());
        let mut response = response_shell(&id, &self.model, status, completed, usage);
        response["incomplete_details"] = incomplete_details;
        let event_type = if status == "completed" {
            "response.completed"
        } else {
            "response.incomplete"
        };
        self.emit_response(&mut output, json!({"type":event_type,"response":response}));
        output.extend_from_slice(b"data: [DONE]\n\n");
        Ok(output)
    }

    fn project_history_response(&self) -> StreamedChatResponse {
        let content = self
            .text
            .as_ref()
            .map(|text| text.value.clone())
            .unwrap_or_default();
        let reasoning = self
            .reasoning
            .as_ref()
            .map(|reasoning| reasoning.value.clone())
            .unwrap_or_default();
        let tool_calls = self
            .tools
            .values()
            .map(|tool| {
                json!({
                    "id": tool.id,
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "arguments": tool.arguments
                    }
                })
            })
            .collect::<Vec<_>>();
        StreamedChatResponse {
            response_id: self.response_id(),
            assistant: json!({
                "role": "assistant",
                "content": content,
                "reasoning_content": reasoning,
                "tool_calls": tool_calls
            }),
        }
    }

    fn finish_anthropic(&mut self) -> Result<Vec<u8>, GatewayError> {
        let mut output = Vec::new();
        if let Some(text) = self.text.take() {
            output.extend(named_event(
                "content_block_stop",
                json!({"type":"content_block_stop","index":text.output_index}),
            )?);
        }
        let tools = std::mem::take(&mut self.tools);
        let has_tools = !tools.is_empty();
        for (_, tool) in tools {
            output.extend(named_event("content_block_stop", json!({"type":"content_block_stop","index":tool.output_index.ok_or(GatewayError::InvalidUpstreamResponse)?}))?);
        }
        let output_tokens = self
            .usage
            .as_ref()
            .and_then(|usage| usage.get("completion_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let stop_reason = anthropic_stop_reason(self.finish_reason.as_deref(), has_tools);
        output.extend(named_event("message_delta", json!({"type":"message_delta","delta":{"stop_reason":stop_reason,"stop_sequence":null},"usage":{"output_tokens":output_tokens}}))?);
        output.extend(named_event("message_stop", json!({"type":"message_stop"}))?);
        Ok(output)
    }

    fn allocate_output(&mut self) -> usize {
        let value = self.next_output_index;
        self.next_output_index += 1;
        value
    }

    fn emit_response(&mut self, output: &mut Vec<u8>, mut value: Value) {
        value["sequence_number"] = json!(self.sequence);
        self.sequence += 1;
        if let Ok(mut event) = data_event(value) {
            output.append(&mut event);
        }
    }
}

fn append_tool_fields(tool: &mut ToolState, call: &Value) {
    if let Some(value) = call.get("id").and_then(Value::as_str) {
        tool.id.push_str(value);
    }
    if let Some(value) = call.pointer("/function/name").and_then(Value::as_str) {
        tool.name.push_str(value);
    }
    if let Some(value) = call.pointer("/function/arguments").and_then(Value::as_str) {
        tool.arguments.push_str(value);
    }
}

fn next_event_boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    let lf = bytes.windows(2).position(|window| window == b"\n\n");
    let crlf = bytes.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(left), Some(right)) if left <= right => Some((left, 2)),
        (Some(_), Some(right)) => Some((right, 4)),
        (Some(left), None) => Some((left, 2)),
        (None, Some(right)) => Some((right, 4)),
        (None, None) => None,
    }
}

fn event_data(event: &[u8]) -> Result<Option<String>, GatewayError> {
    let text = std::str::from_utf8(event).map_err(|_| GatewayError::InvalidUpstreamResponse)?;
    let lines = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
        .collect::<Vec<_>>();
    Ok((!lines.is_empty()).then(|| lines.join("\n")))
}

fn data_event(value: Value) -> Result<Vec<u8>, GatewayError> {
    serde_json::to_vec(&value)
        .map(|json| [b"data: ".as_slice(), json.as_slice(), b"\n\n"].concat())
        .map_err(|_| GatewayError::InvalidUpstreamResponse)
}

fn named_event(name: &str, value: Value) -> Result<Vec<u8>, GatewayError> {
    serde_json::to_vec(&value)
        .map(|json| {
            [
                b"event: ".as_slice(),
                name.as_bytes(),
                b"\ndata: ".as_slice(),
                json.as_slice(),
                b"\n\n",
            ]
            .concat()
        })
        .map_err(|_| GatewayError::InvalidUpstreamResponse)
}

fn response_shell(id: &str, model: &str, status: &str, output: Vec<Value>, usage: Value) -> Value {
    json!({
        "id":id,"object":"response","created_at":unix_seconds(),"status":status,
        "error":null,"incomplete_details":null,"model":model,"output":output,
        "parallel_tool_calls":true,"usage":usage
    })
}

fn responses_usage(usage: Option<&Value>) -> Value {
    let input = usage
        .and_then(|value| value.get("prompt_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output = usage
        .and_then(|value| value.get("completion_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    json!({"input_tokens":input,"input_tokens_details":{"cached_tokens":0},"output_tokens":output,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":input + output})
}

fn anthropic_stop_reason(finish_reason: Option<&str>, has_tools: bool) -> &'static str {
    if has_tools || finish_reason == Some("tool_calls") {
        "tool_use"
    } else {
        match finish_reason {
            Some("length") => "max_tokens",
            Some("content_filter") => "refusal",
            _ => "end_turn",
        }
    }
}

fn responses_completion(finish_reason: Option<&str>) -> (&'static str, Value) {
    match finish_reason {
        Some("length") => ("incomplete", json!({"reason":"max_output_tokens"})),
        Some("content_filter") => ("incomplete", json!({"reason":"content_filter"})),
        _ => ("completed", Value::Null),
    }
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(client_protocol: ClientProtocol) -> PreparedGatewayRequest {
        PreparedGatewayRequest {
            client_protocol,
            upstream_protocol: UpstreamProtocol::OpenAiChatCompletions,
            endpoint: "http://127.0.0.1/v1/chat/completions".to_owned(),
            credential_provider: crate::credential::LlmApiKeyProvider::Kimi,
            credential_style: crate::model::llm_gateway::CredentialStyle::Bearer,
            body: Vec::new(),
            stream: true,
            requested_model: "requested".to_owned(),
            upstream_model: "upstream".to_owned(),
            history_messages: None,
        }
    }

    #[test]
    fn responses_conversion_emits_delta_before_upstream_completion() {
        let mut transformer =
            GatewayStreamTransformer::new(&request(ClientProtocol::OpenAiResponses)).unwrap();
        let first = transformer
            .push(b"data: {\"id\":\"one\",\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\n")
            .unwrap();
        assert!(String::from_utf8_lossy(&first).contains("response.output_text.delta"));
        let last = transformer
            .push(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
            .unwrap();
        assert!(String::from_utf8_lossy(&last).contains("response.completed"));
    }

    #[test]
    fn anthropic_conversion_preserves_length_stop_reason() {
        let mut transformer =
            GatewayStreamTransformer::new(&request(ClientProtocol::AnthropicMessages)).unwrap();
        transformer
            .push(b"data: {\"id\":\"one\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n")
            .unwrap();
        let last = transformer
            .push(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n")
            .unwrap();
        assert!(String::from_utf8_lossy(&last).contains("max_tokens"));
    }

    #[test]
    fn responses_conversion_emits_incomplete_for_token_limit() {
        let mut transformer =
            GatewayStreamTransformer::new(&request(ClientProtocol::OpenAiResponses)).unwrap();
        transformer
            .push(b"data: {\"id\":\"one\",\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n")
            .unwrap();
        let last = transformer
            .push(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n")
            .unwrap();
        let text = String::from_utf8_lossy(&last);
        assert!(text.contains("response.incomplete"));
        assert!(text.contains("max_output_tokens"));
    }
}
