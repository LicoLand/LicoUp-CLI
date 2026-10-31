//! Incremental SSE protocol conversion for the local LLM gateway.

use crate::model::llm_gateway::{
    ClientProtocol, GatewayError, MAX_GATEWAY_BODY_BYTES, PreparedGatewayRequest, UpstreamProtocol,
    client_wire_dialect, sse_frame, upstream_stream_decoder,
};
use a4mp::wire::{self, StreamEncoder, WireDialect};
use a4mp::{SseEvent, SseEventDecoder, StreamEvent};
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
    /// Responses client over a Chat upstream: the local interim projection
    /// until a4mp exposes a Responses wire dialect.
    Responses(ChatStreamState),
    /// Cross-dialect relay: upstream protocol decoder → canonical
    /// `StreamEvent`s → client wire encoder, all supplied by a4mp.
    Canonical(CanonicalRelay),
    /// Responses client over a non-Chat upstream: canonical events rendered
    /// as Chat SSE, then projected through the local interim.
    CanonicalResponses(CanonicalRelay, ChatStreamState),
}

struct CanonicalRelay {
    decoder: Box<dyn SseEventDecoder>,
    encoder: Box<dyn StreamEncoder>,
    started: bool,
}

impl CanonicalRelay {
    fn new(upstream: UpstreamProtocol, dialect: WireDialect, model: &str) -> Self {
        Self {
            decoder: upstream_stream_decoder(upstream),
            encoder: wire::stream_encoder(dialect, model),
            started: false,
        }
    }

    fn decode(&mut self, frame: &SseEvent) -> Result<Vec<StreamEvent>, GatewayError> {
        self.decoder
            .decode(frame)
            .map_err(|_| GatewayError::InvalidUpstreamResponse)
    }

    fn encode(&mut self, events: &[StreamEvent]) -> Vec<SseEvent> {
        let mut frames = Vec::new();
        if !self.started {
            self.started = true;
            frames.extend(self.encoder.preamble());
        }
        for event in events {
            frames.extend(self.encoder.encode(event));
        }
        let _ = self.encoder.drain_losses();
        frames
    }

    fn finish(&mut self) -> Vec<SseEvent> {
        let mut frames = self.encode(&[]);
        frames.extend(self.encoder.finish());
        frames
    }
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
        let relay = |upstream: UpstreamProtocol, dialect: WireDialect| {
            CanonicalRelay::new(upstream, dialect, &request.requested_model)
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
            (ClientProtocol::OpenAiResponses, upstream) => StreamMode::CanonicalResponses(
                relay(upstream, WireDialect::OpenAiChatCompletions),
                state(),
            ),
            (client, upstream) => StreamMode::Canonical(relay(
                upstream,
                client_wire_dialect(client).ok_or(GatewayError::UnsupportedConversion)?,
            )),
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
            let frame = parse_frame(&event)?;
            if frame.data.is_empty() && frame.event.is_none() {
                continue;
            }
            if frame.data.trim() == "[DONE]" {
                output.extend(self.finalize()?);
                self.finished = true;
                continue;
            }
            match &mut self.mode {
                StreamMode::Passthrough => unreachable!(),
                StreamMode::Responses(state) => {
                    let chunk: Value = serde_json::from_str(&frame.data)
                        .map_err(|_| GatewayError::InvalidUpstreamResponse)?;
                    output.extend(state.responses_chunk(&chunk)?);
                }
                StreamMode::Canonical(relay) => {
                    let events = relay.decode(&frame)?;
                    for out in relay.encode(&events) {
                        output.extend_from_slice(&sse_frame(&out));
                    }
                }
                StreamMode::CanonicalResponses(relay, state) => {
                    let events = relay.decode(&frame)?;
                    for out in relay.encode(&events) {
                        output.extend(state.responses_frame(&out)?);
                    }
                }
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
            StreamMode::Responses(state) | StreamMode::CanonicalResponses(_, state) => {
                state.history_response.take()
            }
            StreamMode::Canonical(_) | StreamMode::Passthrough => None,
        }
    }

    fn finalize(&mut self) -> Result<Vec<u8>, GatewayError> {
        match &mut self.mode {
            StreamMode::Responses(state) => state.finish_responses(),
            StreamMode::Canonical(relay) => Ok(relay.finish().iter().flat_map(sse_frame).collect()),
            StreamMode::CanonicalResponses(relay, state) => {
                let mut output = Vec::new();
                for frame in relay.finish() {
                    output.extend(state.responses_frame(&frame)?);
                }
                output.extend(state.finish_responses()?);
                Ok(output)
            }
            StreamMode::Passthrough => Ok(Vec::new()),
        }
    }
}

impl ChatStreamState {
    /// One Chat SSE frame produced by the canonical relay's OpenAI encoder is
    /// folded back into the interim projection.
    fn responses_frame(&mut self, frame: &SseEvent) -> Result<Vec<u8>, GatewayError> {
        let data = frame.data.trim();
        if data.is_empty() || data == "[DONE]" {
            return Ok(Vec::new());
        }
        let chunk: Value =
            serde_json::from_str(data).map_err(|_| GatewayError::InvalidUpstreamResponse)?;
        self.responses_chunk(&chunk)
    }

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

fn parse_frame(raw: &[u8]) -> Result<SseEvent, GatewayError> {
    let text = std::str::from_utf8(raw).map_err(|_| GatewayError::InvalidUpstreamResponse)?;
    let mut frame = SseEvent::default();
    let mut data = Vec::new();
    for line in text.lines() {
        if line.starts_with(':') {
            continue;
        }
        if let Some(value) = line.strip_prefix("data:") {
            data.push(value.trim_start().to_owned());
        } else if let Some(value) = line.strip_prefix("event:") {
            frame.event = Some(value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("id:") {
            frame.id = Some(value.trim().to_owned());
        }
    }
    frame.data = data.join("\n");
    Ok(frame)
}

fn data_event(value: Value) -> Result<Vec<u8>, GatewayError> {
    serde_json::to_vec(&value)
        .map(|json| [b"data: ".as_slice(), json.as_slice(), b"\n\n"].concat())
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

    /// A Responses client on an Anthropic upstream rides the canonical relay:
    /// Anthropic SSE is decoded to canonical events, re-encoded as Chat SSE,
    /// and projected into Responses events by the interim.
    #[test]
    fn responses_client_on_anthropic_upstream_bridges_through_canonical() {
        let mut request = request(ClientProtocol::OpenAiResponses);
        request.upstream_protocol = UpstreamProtocol::AnthropicMessages;
        let mut transformer = GatewayStreamTransformer::new(&request).unwrap();
        let first = transformer
            .push(
                concat!(
                    "event: message_start\n",
                    "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"role\":\"assistant\",\"content\":[],\"usage\":{\"input_tokens\":3}}}\n\n",
                    "event: content_block_start\n",
                    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                    "event: content_block_delta\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
                )
                .as_bytes(),
            )
            .unwrap();
        let last = transformer
            .push(
                concat!(
                    "event: content_block_stop\n",
                    "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                    "event: message_delta\n",
                    "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
                    "event: message_stop\n",
                    "data: {\"type\":\"message_stop\"}\n\n",
                )
                .as_bytes(),
            )
            .unwrap();
        let combined = [first, last].concat();
        let text = String::from_utf8_lossy(&combined);
        assert!(text.contains("response.output_text.delta") || text.contains("response.completed"));

        let tail = transformer.finish().unwrap();
        let text = String::from_utf8_lossy(&tail);
        assert!(text.contains("response.completed") || text.contains("response.output_text.done"));
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
