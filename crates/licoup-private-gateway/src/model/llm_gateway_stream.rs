//! Client-facing stream relay for the local LLM gateway.
//!
//! The forwarding core decodes upstream SSE into canonical `StreamEvent`s;
//! this shell owns the dialect the client sees and the Responses-history
//! projection that keeps `previous_response_id` continuable on non-Responses
//! upstreams. Each canonical event is re-encoded by a4mp's `StreamEncoder`,
//! so the sink receives protocol-complete frames in the client's own dialect.

use crate::model::llm_gateway::{
    ClientProtocol, PreparedGatewayRequest, UpstreamProtocol, client_wire_dialect, sse_frame,
};
use a4mp::wire::{StreamEncoder, stream_encoder};
use a4mp::{
    ContentPart, Message, PartKind, Protocol, ReasoningBlock, Role, SseEvent, StreamEvent, ToolCall,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A completed streamed assistant turn, kept for `previous_response_id` replay.
pub struct StreamedChatResponse {
    pub response_id: String,
    pub assistant: Message,
}

/// Canonical `StreamEvent`s → client-dialect SSE bytes, with an optional
/// history accumulator for Responses clients on foreign upstreams.
pub(crate) struct ClientStreamRelay {
    encoder: Box<dyn StreamEncoder>,
    history: Option<StreamHistory>,
    /// Whether usage must reach the encoder before its terminal event.
    /// Responses and Messages fold usage into `response.completed` /
    /// `message_delta`; a Chat client's own wire order reports the trailing
    /// usage chunk after `finish_reason`, so it is left untouched.
    usage_before_terminal: bool,
    /// A terminal `Finished` held one event slot: upstreams that emit usage
    /// in a separate trailing chunk (OpenAI `stream_options.include_usage`)
    /// deliver it after `Finished`, where the dialect encoder would record a
    /// loss instead of folding it into the terminal frame.
    pending_terminal: Option<StreamEvent>,
}

struct StreamHistory {
    response_id: Option<String>,
    open: BTreeMap<u32, OpenPart>,
    done: BTreeMap<u32, ContentPart>,
    /// Reasoning replay is origin-bound: only a Chat upstream's reasoning can
    /// be rebuilt as replayable opaque state (`reasoning_content`).
    chat_replay: bool,
}

enum OpenPart {
    Text(String),
    Reasoning(String),
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
    Other,
}

impl ClientStreamRelay {
    pub(crate) fn new(request: &PreparedGatewayRequest) -> Self {
        let history = (request.client_protocol == ClientProtocol::OpenAiResponses
            && request.upstream_protocol != UpstreamProtocol::OpenAiResponses)
            .then(|| StreamHistory {
                response_id: None,
                open: BTreeMap::new(),
                done: BTreeMap::new(),
                chat_replay: request.upstream_protocol == UpstreamProtocol::OpenAiChatCompletions,
            });
        Self {
            encoder: stream_encoder(
                client_wire_dialect(request.client_protocol),
                &request.requested_model,
            ),
            history,
            usage_before_terminal: request.client_protocol != ClientProtocol::OpenAiChatCompletions,
            pending_terminal: None,
        }
    }

    pub(crate) fn preamble(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        for frame in self.encoder.preamble() {
            self.observe_frame(&frame);
            out.extend_from_slice(&sse_frame(&frame));
        }
        out
    }

    pub(crate) fn push(&mut self, event: &StreamEvent) -> Vec<u8> {
        if let Some(history) = self.history.as_mut() {
            history.observe(event);
        }
        let mut out = Vec::new();
        if self.usage_before_terminal {
            match event {
                StreamEvent::Finished { .. } => {
                    if let Some(terminal) = self.pending_terminal.take() {
                        self.encode_into(&mut out, &terminal);
                    }
                    self.pending_terminal = Some(event.clone());
                    return out;
                }
                StreamEvent::Usage(_) => {
                    // Usage trailing the terminal event: fold it in first so
                    // the terminal frame carries it.
                    self.encode_into(&mut out, event);
                    if let Some(terminal) = self.pending_terminal.take() {
                        self.encode_into(&mut out, &terminal);
                    }
                    return out;
                }
                _ => {}
            }
            // Any other event after a deferred terminal keeps wire order.
            if let Some(terminal) = self.pending_terminal.take() {
                self.encode_into(&mut out, &terminal);
            }
        }
        self.encode_into(&mut out, event);
        out
    }

    fn encode_into(&mut self, out: &mut Vec<u8>, event: &StreamEvent) {
        for frame in self.encoder.encode(event) {
            self.observe_frame(&frame);
            out.extend_from_slice(&sse_frame(&frame));
        }
    }

    pub(crate) fn finish(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(terminal) = self.pending_terminal.take() {
            self.encode_into(&mut out, &terminal);
        }
        for frame in self.encoder.finish() {
            self.observe_frame(&frame);
            out.extend_from_slice(&sse_frame(&frame));
        }
        let _ = self.encoder.drain_losses();
        out
    }

    /// The Responses encoder names its own response id; it is announced on the
    /// `response.created`/`response.in_progress` envelopes — capture it so
    /// stored history answers to the same id the client saw.
    fn observe_frame(&mut self, frame: &SseEvent) {
        let Some(history) = self.history.as_mut() else {
            return;
        };
        if history.response_id.is_some() || frame.event.as_deref() != Some("response.created") {
            return;
        }
        if let Ok(data) = serde_json::from_str::<Value>(&frame.data) {
            history.response_id = data
                .pointer("/response/id")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
    }

    pub(crate) fn take_history(&mut self) -> Option<StreamedChatResponse> {
        let mut history = self.history.take()?;
        for (index, part) in std::mem::take(&mut history.open) {
            if let Some(part) = part.finish(history.chat_replay) {
                history.done.insert(index, part);
            }
        }
        let response_id = history
            .response_id
            .unwrap_or_else(|| format!("resp_{}", unix_seconds()));
        Some(StreamedChatResponse {
            response_id,
            assistant: Message {
                role: Role::Assistant,
                content: history.done.into_values().collect(),
            },
        })
    }
}

impl StreamHistory {
    fn observe(&mut self, event: &StreamEvent) {
        match event {
            StreamEvent::PartStarted { index, kind } => {
                self.open.entry(*index).or_insert_with(|| match kind {
                    PartKind::Text => OpenPart::Text(String::new()),
                    PartKind::Reasoning => OpenPart::Reasoning(String::new()),
                    PartKind::ToolCall => OpenPart::ToolCall {
                        id: String::new(),
                        name: String::new(),
                        arguments: String::new(),
                    },
                    _ => OpenPart::Other,
                });
            }
            StreamEvent::TextDelta { index, text } => {
                if let Some(OpenPart::Text(buffer)) = self.open.get_mut(index) {
                    buffer.push_str(text);
                }
            }
            StreamEvent::ReasoningDelta { index, text } => {
                if let Some(OpenPart::Reasoning(buffer)) = self.open.get_mut(index) {
                    buffer.push_str(text);
                }
            }
            StreamEvent::ToolCallStarted { index, id, name } => {
                self.open.insert(
                    *index,
                    OpenPart::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: String::new(),
                    },
                );
            }
            StreamEvent::ToolCallDelta { index, arguments } => match self.open.get_mut(index) {
                Some(OpenPart::ToolCall {
                    arguments: buffer, ..
                }) => buffer.push_str(arguments),
                Some(_) => {}
                None => {
                    self.open.insert(
                        *index,
                        OpenPart::ToolCall {
                            id: String::new(),
                            name: String::new(),
                            arguments: arguments.clone(),
                        },
                    );
                }
            },
            StreamEvent::PartFinished { index } => {
                if let Some(part) = self.open.remove(index)
                    && let Some(part) = part.finish(self.chat_replay)
                {
                    self.done.insert(*index, part);
                }
            }
            _ => {}
        }
    }
}

impl OpenPart {
    /// The canonical part this accumulated stream yields for history replay,
    /// or `None` when the part cannot be faithfully reconstructed (opaque
    /// reasoning signatures, provider-native blocks, media).
    fn finish(self, chat_replay: bool) -> Option<ContentPart> {
        match self {
            OpenPart::Text(text) if !text.is_empty() => Some(ContentPart::Text { text }),
            OpenPart::Reasoning(text) if chat_replay && !text.is_empty() => {
                Some(ContentPart::Reasoning(ReasoningBlock {
                    protocol: Protocol::OpenAiChatCompletions,
                    opaque: json!({"reasoning_content": text}),
                    summary: Some(text),
                }))
            }
            OpenPart::ToolCall {
                id,
                name,
                arguments,
            } if !name.is_empty() => Some(ContentPart::ToolCall(ToolCall {
                id,
                name,
                arguments: serde_json::from_str(&arguments).unwrap_or(Value::String(arguments)),
            })),
            _ => None,
        }
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
    use crate::credential::LlmApiKeyProvider;
    use crate::model::llm_gateway::{
        ClientProtocol, CompiledGateway, CredentialStyle, GatewayConfig, GatewayProvider,
        ModelRoute,
    };

    fn request(client: ClientProtocol, upstream: UpstreamProtocol) -> PreparedGatewayRequest {
        let provider_id = match upstream {
            UpstreamProtocol::OpenAiChatCompletions => "chat-lane",
            UpstreamProtocol::AnthropicMessages => "anthropic-lane",
            UpstreamProtocol::OpenAiResponses => "responses-lane",
        };
        let gateway = CompiledGateway::compile(GatewayConfig {
            schema_version: 1,
            providers: vec![
                GatewayProvider {
                    id: "chat-lane".into(),
                    base_url: "https://chat.example/v1".into(),
                    protocol: UpstreamProtocol::OpenAiChatCompletions,
                    credential_provider: LlmApiKeyProvider::Kimi,
                    credential_style: CredentialStyle::Bearer,
                },
                GatewayProvider {
                    id: "anthropic-lane".into(),
                    base_url: "https://anthropic.example".into(),
                    protocol: UpstreamProtocol::AnthropicMessages,
                    credential_provider: LlmApiKeyProvider::DeepSeek,
                    credential_style: CredentialStyle::XApiKey,
                },
                GatewayProvider {
                    id: "responses-lane".into(),
                    base_url: "https://responses.example/v1".into(),
                    protocol: UpstreamProtocol::OpenAiResponses,
                    credential_provider: LlmApiKeyProvider::Kilo,
                    credential_style: CredentialStyle::Bearer,
                },
            ],
            routes: vec![ModelRoute {
                client_protocol: client,
                requested_model: "requested".into(),
                provider_id: provider_id.into(),
                upstream_model: "upstream".into(),
            }],
        })
        .unwrap();
        let path = match client {
            ClientProtocol::AnthropicMessages => "/v1/messages",
            ClientProtocol::OpenAiChatCompletions => "/v1/chat/completions",
            ClientProtocol::OpenAiResponses => "/v1/responses",
        };
        let body = match client {
            ClientProtocol::OpenAiResponses => {
                br#"{"model":"requested","input":"go","stream":true}"#.as_slice()
            }
            _ => br#"{"model":"requested","messages":[],"stream":true}"#.as_slice(),
        };
        gateway.prepare(path, body).unwrap()
    }

    fn drive(relay: &mut ClientStreamRelay, events: &[StreamEvent]) -> Vec<u8> {
        let mut out = relay.preamble();
        for event in events {
            out.extend_from_slice(&relay.push(event));
        }
        out.extend_from_slice(&relay.finish());
        out
    }

    #[test]
    fn responses_client_receives_responses_events_and_history() {
        let mut relay = ClientStreamRelay::new(&request(
            ClientProtocol::OpenAiResponses,
            UpstreamProtocol::OpenAiChatCompletions,
        ));
        let bytes = drive(
            &mut relay,
            &[
                StreamEvent::PartStarted {
                    index: 0,
                    kind: PartKind::Reasoning,
                },
                StreamEvent::ReasoningDelta {
                    index: 0,
                    text: "thought".to_owned(),
                },
                StreamEvent::PartFinished { index: 0 },
                StreamEvent::PartStarted {
                    index: 1,
                    kind: PartKind::Text,
                },
                StreamEvent::TextDelta {
                    index: 1,
                    text: "hi".to_owned(),
                },
                StreamEvent::PartFinished { index: 1 },
                StreamEvent::ToolCallStarted {
                    index: 2,
                    id: "call-1".to_owned(),
                    name: "lookup".to_owned(),
                },
                StreamEvent::ToolCallDelta {
                    index: 2,
                    arguments: "{\"q\":\"fixture\"}".to_owned(),
                },
                StreamEvent::PartFinished { index: 2 },
                StreamEvent::Finished { stop: None },
            ],
        );
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("response.created"));
        assert!(text.contains("response.output_item.added"));
        assert!(text.contains("response.output_text.delta"));
        assert!(text.contains("response.function_call_arguments.delta"));
        assert!(text.contains("response.completed"));

        let history = relay.take_history().expect("responses history recorded");
        assert!(history.response_id.starts_with("resp_"));
        assert!(history.assistant.content.iter().any(|part| matches!(
            part,
            ContentPart::Text { text } if text == "hi"
        )));
        assert!(history.assistant.content.iter().any(|part| matches!(
            part,
            ContentPart::Reasoning(block)
                if block.summary.as_deref() == Some("thought")
                    && block.opaque["reasoning_content"] == "thought"
        )));
        assert!(history.assistant.content.iter().any(|part| matches!(
            part,
            ContentPart::ToolCall(call)
                if call.id == "call-1" && call.name == "lookup"
                    && call.arguments == json!({"q":"fixture"})
        )));
    }

    #[test]
    fn responses_client_usage_trailing_finished_folds_into_completed() {
        // Chat upstreams emit the terminal finish_reason chunk first and a
        // separate usage chunk last; the usage must still land inside
        // `response.completed` rather than be lost behind the terminal event.
        let mut relay = ClientStreamRelay::new(&request(
            ClientProtocol::OpenAiResponses,
            UpstreamProtocol::OpenAiChatCompletions,
        ));
        let bytes = drive(
            &mut relay,
            &[
                StreamEvent::PartStarted {
                    index: 0,
                    kind: PartKind::Text,
                },
                StreamEvent::TextDelta {
                    index: 0,
                    text: "hi".to_owned(),
                },
                StreamEvent::PartFinished { index: 0 },
                StreamEvent::Finished {
                    stop: Some(a4mp::StopReason::Complete),
                },
                StreamEvent::Usage(a4mp::Usage {
                    input_tokens: Some(12),
                    output_tokens: Some(3),
                    extensions: Default::default(),
                }),
            ],
        );
        let text = String::from_utf8(bytes).unwrap();
        let completed = text
            .split("\n\n")
            .find(|frame| frame.contains("response.completed"))
            .expect("stream completes");
        assert!(completed.contains("\"usage\""), "{completed}");
        assert!(completed.contains("\"input_tokens\":12"), "{completed}");
        assert!(completed.contains("\"output_tokens\":3"), "{completed}");
    }

    #[test]
    fn chat_client_keeps_finish_chunk_before_trailing_usage_chunk() {
        let mut relay = ClientStreamRelay::new(&request(
            ClientProtocol::OpenAiChatCompletions,
            UpstreamProtocol::OpenAiChatCompletions,
        ));
        let bytes = drive(
            &mut relay,
            &[
                StreamEvent::PartStarted {
                    index: 0,
                    kind: PartKind::Text,
                },
                StreamEvent::TextDelta {
                    index: 0,
                    text: "hi".to_owned(),
                },
                StreamEvent::PartFinished { index: 0 },
                StreamEvent::Finished {
                    stop: Some(a4mp::StopReason::Complete),
                },
                StreamEvent::Usage(a4mp::Usage {
                    input_tokens: Some(12),
                    output_tokens: Some(3),
                    extensions: Default::default(),
                }),
            ],
        );
        let text = String::from_utf8(bytes).unwrap();
        let finish = text.find("\"finish_reason\"").expect("finish chunk");
        let usage = text.find("\"usage\"").expect("usage chunk");
        assert!(
            finish < usage,
            "chat wire order stays finish-chunk then usage-chunk"
        );
    }

    #[test]
    fn anthropic_upstream_reasoning_is_not_replayable_from_a_stream() {
        let mut relay = ClientStreamRelay::new(&request(
            ClientProtocol::OpenAiResponses,
            UpstreamProtocol::AnthropicMessages,
        ));
        drive(
            &mut relay,
            &[
                StreamEvent::PartStarted {
                    index: 0,
                    kind: PartKind::Reasoning,
                },
                StreamEvent::ReasoningDelta {
                    index: 0,
                    text: "thought".to_owned(),
                },
                StreamEvent::PartFinished { index: 0 },
                StreamEvent::PartStarted {
                    index: 1,
                    kind: PartKind::Text,
                },
                StreamEvent::TextDelta {
                    index: 1,
                    text: "hi".to_owned(),
                },
                StreamEvent::PartFinished { index: 1 },
                StreamEvent::Finished { stop: None },
            ],
        );
        let history = relay.take_history().expect("responses history recorded");
        assert!(
            history
                .assistant
                .content
                .iter()
                .all(|part| !matches!(part, ContentPart::Reasoning(_))),
            "streamed Anthropic reasoning cannot be rebuilt as opaque state"
        );
        assert!(history.assistant.content.iter().any(|part| matches!(
            part,
            ContentPart::Text { text } if text == "hi"
        )));
    }

    #[test]
    fn anthropic_client_receives_messages_dialect_events() {
        let mut relay = ClientStreamRelay::new(&request(
            ClientProtocol::AnthropicMessages,
            UpstreamProtocol::OpenAiChatCompletions,
        ));
        let bytes = drive(
            &mut relay,
            &[
                StreamEvent::PartStarted {
                    index: 0,
                    kind: PartKind::Text,
                },
                StreamEvent::TextDelta {
                    index: 0,
                    text: "hi".to_owned(),
                },
                StreamEvent::PartFinished { index: 0 },
                StreamEvent::Finished {
                    stop: Some(a4mp::StopReason::Length),
                },
            ],
        );
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("content_block_start"));
        assert!(text.contains("text_delta"));
        assert!(text.contains("message_stop") || text.contains("max_tokens"));
        assert!(relay.take_history().is_none());
    }

    #[test]
    fn chat_client_receives_chat_dialect_events() {
        let mut relay = ClientStreamRelay::new(&request(
            ClientProtocol::OpenAiChatCompletions,
            UpstreamProtocol::AnthropicMessages,
        ));
        let bytes = drive(
            &mut relay,
            &[
                StreamEvent::PartStarted {
                    index: 0,
                    kind: PartKind::Text,
                },
                StreamEvent::TextDelta {
                    index: 0,
                    text: "hi".to_owned(),
                },
                StreamEvent::PartFinished { index: 0 },
                StreamEvent::Finished { stop: None },
            ],
        );
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("chat.completion.chunk"));
        assert!(text.contains("\"content\":\"hi\"") || text.contains("\"content\": \"hi\""));
        assert!(text.contains("[DONE]"));
    }
}
