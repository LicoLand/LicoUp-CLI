//! Closed model routing and protocol conversion for the local LLM gateway.
//!
//! Configuration contains credential references only. Secrets are resolved by
//! the platform transport after routing and endpoint validation succeed.

use licoup_vault::api_key::LlmApiKeyProvider;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Mutex;
use url::{Host, Url};

pub const MAX_GATEWAY_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_CONFIG_ENTRIES: usize = 256;
const MAX_ID_BYTES: usize = 256;
const MAX_HISTORY_ENTRIES: usize = 64;
const MAX_HISTORY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientProtocol {
    AnthropicMessages,
    OpenAiChatCompletions,
    OpenAiResponses,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamProtocol {
    AnthropicMessages,
    OpenAiChatCompletions,
    OpenAiResponses,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStyle {
    Bearer,
    XApiKey,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GatewayProvider {
    pub id: String,
    pub base_url: String,
    pub protocol: UpstreamProtocol,
    pub credential_provider: LlmApiKeyProvider,
    pub credential_style: CredentialStyle,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelRoute {
    pub client_protocol: ClientProtocol,
    pub requested_model: String,
    pub provider_id: String,
    pub upstream_model: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GatewayConfig {
    pub schema_version: u32,
    pub providers: Vec<GatewayProvider>,
    pub routes: Vec<ModelRoute>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatewayError {
    InvalidConfig,
    UnsupportedPath,
    InvalidRequest,
    RequestTooLarge,
    RouteNotFound,
    UnsupportedConversion,
    InvalidUpstreamResponse,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedGatewayRequest {
    pub client_protocol: ClientProtocol,
    pub upstream_protocol: UpstreamProtocol,
    pub endpoint: String,
    pub credential_provider: LlmApiKeyProvider,
    pub credential_style: CredentialStyle,
    pub body: Vec<u8>,
    pub stream: bool,
    pub requested_model: String,
    pub upstream_model: String,
    pub(crate) history_messages: Option<Vec<Value>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatewayResponse {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

pub struct CompiledGateway {
    providers: HashMap<String, GatewayProvider>,
    routes: HashMap<(ClientProtocol, String), ModelRoute>,
    history: Mutex<ResponseHistory>,
}

#[derive(Default)]
struct ResponseHistory {
    entries: HashMap<String, (Vec<Value>, usize)>,
    order: VecDeque<String>,
    total_bytes: usize,
}

impl CompiledGateway {
    pub fn compile(config: GatewayConfig) -> Result<Self, GatewayError> {
        if config.schema_version != 1
            || config.providers.len() > MAX_CONFIG_ENTRIES
            || config.routes.len() > MAX_CONFIG_ENTRIES
        {
            return Err(GatewayError::InvalidConfig);
        }
        let mut providers = HashMap::with_capacity(config.providers.len());
        for provider in config.providers {
            validate_provider(&provider)?;
            if providers.insert(provider.id.clone(), provider).is_some() {
                return Err(GatewayError::InvalidConfig);
            }
        }
        let mut routes = HashMap::with_capacity(config.routes.len());
        for route in config.routes {
            if !valid_id(&route.requested_model)
                || !valid_id(&route.provider_id)
                || !valid_id(&route.upstream_model)
                || !providers.contains_key(&route.provider_id)
            {
                return Err(GatewayError::InvalidConfig);
            }
            let key = (route.client_protocol, route.requested_model.clone());
            if routes.insert(key, route).is_some() {
                return Err(GatewayError::InvalidConfig);
            }
        }
        Ok(Self {
            providers,
            routes,
            history: Mutex::new(ResponseHistory::default()),
        })
    }

    /// Providers whose live model catalogs may be queried. Stable sorting keeps
    /// a multi-provider `/v1/models` response deterministic.
    pub fn catalog_providers(&self) -> Vec<GatewayProvider> {
        let mut providers = self.providers.values().cloned().collect::<Vec<_>>();
        providers.sort_by(|left, right| left.id.cmp(&right.id));
        providers
    }

    pub fn prepare(&self, path: &str, body: &[u8]) -> Result<PreparedGatewayRequest, GatewayError> {
        if body.len() > MAX_GATEWAY_BODY_BYTES {
            return Err(GatewayError::RequestTooLarge);
        }
        let client_protocol = match path.trim_end_matches('/') {
            "/v1/messages" => ClientProtocol::AnthropicMessages,
            "/v1/chat/completions" | "/chat/completions" => ClientProtocol::OpenAiChatCompletions,
            "/v1/responses" | "/responses" => ClientProtocol::OpenAiResponses,
            _ => return Err(GatewayError::UnsupportedPath),
        };
        let mut document: Value =
            serde_json::from_slice(body).map_err(|_| GatewayError::InvalidRequest)?;
        let requested_model = document
            .get("model")
            .and_then(Value::as_str)
            .filter(|model| valid_id(model))
            .ok_or(GatewayError::InvalidRequest)?
            .to_owned();
        let route = self
            .routes
            .get(&(client_protocol, requested_model.clone()))
            .cloned()
            .or_else(|| self.provider_namespaced_route(client_protocol, &requested_model))
            .ok_or(GatewayError::RouteNotFound)?;
        let provider = self
            .providers
            .get(&route.provider_id)
            .ok_or(GatewayError::InvalidConfig)?;
        let stream = document
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        document["model"] = Value::String(route.upstream_model.clone());
        let previous_messages = if client_protocol == ClientProtocol::OpenAiResponses
            && provider.protocol == UpstreamProtocol::OpenAiChatCompletions
        {
            match document.get("previous_response_id") {
                None | Some(Value::Null) => None,
                Some(Value::String(id)) if valid_id(id) => Some(
                    self.history
                        .lock()
                        .map_err(|_| GatewayError::InvalidRequest)?
                        .get(id)
                        .ok_or(GatewayError::UnsupportedConversion)?,
                ),
                _ => return Err(GatewayError::InvalidRequest),
            }
        } else {
            None
        };
        let converted = match (client_protocol, provider.protocol) {
            (ClientProtocol::AnthropicMessages, UpstreamProtocol::AnthropicMessages)
            | (ClientProtocol::OpenAiChatCompletions, UpstreamProtocol::OpenAiChatCompletions)
            | (ClientProtocol::OpenAiResponses, UpstreamProtocol::OpenAiResponses) => document,
            (ClientProtocol::AnthropicMessages, UpstreamProtocol::OpenAiChatCompletions) => {
                anthropic_to_chat(&document)?
            }
            (ClientProtocol::OpenAiResponses, UpstreamProtocol::OpenAiChatCompletions) => {
                responses_to_chat(&document, previous_messages.as_deref())?
            }
            _ => return Err(GatewayError::UnsupportedConversion),
        };
        let endpoint = endpoint_for(provider)?;
        let history_messages = (client_protocol == ClientProtocol::OpenAiResponses
            && provider.protocol == UpstreamProtocol::OpenAiChatCompletions)
            .then(|| {
                converted
                    .get("messages")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            });
        let body = serde_json::to_vec(&converted).map_err(|_| GatewayError::InvalidRequest)?;
        Ok(PreparedGatewayRequest {
            client_protocol,
            upstream_protocol: provider.protocol,
            endpoint,
            credential_provider: provider.credential_provider,
            credential_style: provider.credential_style,
            body,
            stream,
            requested_model,
            upstream_model: route.upstream_model.clone(),
            history_messages,
        })
    }

    fn provider_namespaced_route(
        &self,
        client_protocol: ClientProtocol,
        requested_model: &str,
    ) -> Option<ModelRoute> {
        let (provider_id, upstream_model) = requested_model.split_once(':')?;
        if namespaced_model_id(provider_id, upstream_model).as_deref() != Some(requested_model)
            || !self.providers.contains_key(provider_id)
        {
            return None;
        }
        Some(ModelRoute {
            client_protocol,
            requested_model: requested_model.to_owned(),
            provider_id: provider_id.to_owned(),
            upstream_model: legacy_generated_alias(provider_id, upstream_model).to_owned(),
        })
    }

    pub fn finish(
        &self,
        request: &PreparedGatewayRequest,
        status: u16,
        upstream_content_type: Option<&str>,
        body: &[u8],
    ) -> Result<GatewayResponse, GatewayError> {
        if body.len() > MAX_GATEWAY_BODY_BYTES {
            return Err(GatewayError::RequestTooLarge);
        }
        if !(200..300).contains(&status) {
            return Ok(normalize_error(status, body));
        }
        let is_sse = upstream_content_type
            .is_some_and(|value| value.to_ascii_lowercase().contains("text/event-stream"));
        let converted = match (request.client_protocol, request.upstream_protocol, is_sse) {
            (ClientProtocol::AnthropicMessages, UpstreamProtocol::AnthropicMessages, _)
            | (ClientProtocol::OpenAiChatCompletions, UpstreamProtocol::OpenAiChatCompletions, _)
            | (ClientProtocol::OpenAiResponses, UpstreamProtocol::OpenAiResponses, _) => {
                return Ok(GatewayResponse {
                    status,
                    content_type: if is_sse {
                        "text/event-stream"
                    } else {
                        "application/json"
                    },
                    body: body.to_vec(),
                });
            }
            (ClientProtocol::AnthropicMessages, UpstreamProtocol::OpenAiChatCompletions, false) => {
                chat_to_anthropic(body, &request.requested_model)?
            }
            (ClientProtocol::OpenAiResponses, UpstreamProtocol::OpenAiChatCompletions, false) => {
                chat_to_responses(body, &request.requested_model)?
            }
            (ClientProtocol::AnthropicMessages, UpstreamProtocol::OpenAiChatCompletions, true) => {
                chat_sse_to_anthropic(body, &request.requested_model)?
            }
            (ClientProtocol::OpenAiResponses, UpstreamProtocol::OpenAiChatCompletions, true) => {
                chat_sse_to_responses(body, &request.requested_model)?
            }
            _ => return Err(GatewayError::UnsupportedConversion),
        };
        if request.client_protocol == ClientProtocol::OpenAiResponses
            && request.upstream_protocol == UpstreamProtocol::OpenAiChatCompletions
        {
            self.remember_chat_response(request, body, is_sse)?;
        }
        Ok(GatewayResponse {
            status,
            content_type: if is_sse {
                "text/event-stream"
            } else {
                "application/json"
            },
            body: converted,
        })
    }

    fn remember_chat_response(
        &self,
        request: &PreparedGatewayRequest,
        body: &[u8],
        is_sse: bool,
    ) -> Result<(), GatewayError> {
        let (response_id, assistant) = if is_sse {
            aggregate_chat_sse(body)?
        } else {
            let chat: Value =
                serde_json::from_slice(body).map_err(|_| GatewayError::InvalidUpstreamResponse)?;
            let assistant = chat
                .pointer("/choices/0/message")
                .cloned()
                .ok_or(GatewayError::InvalidUpstreamResponse)?;
            (response_id(&chat), assistant)
        };
        self.remember_assistant_response(request, response_id, assistant)
    }

    fn remember_assistant_response(
        &self,
        request: &PreparedGatewayRequest,
        response_id: String,
        assistant: Value,
    ) -> Result<(), GatewayError> {
        let mut messages = request.history_messages.clone().unwrap_or_default();
        messages.retain(|message| message.get("role").and_then(Value::as_str) != Some("system"));
        messages.push(assistant);
        self.history
            .lock()
            .map_err(|_| GatewayError::InvalidUpstreamResponse)?
            .put(response_id, messages);
        Ok(())
    }

    pub fn remember_stream_response(
        &self,
        request: &PreparedGatewayRequest,
        response: crate::model::llm_gateway_stream::StreamedChatResponse,
    ) -> Result<(), GatewayError> {
        if request.client_protocol == ClientProtocol::OpenAiResponses
            && request.upstream_protocol == UpstreamProtocol::OpenAiChatCompletions
        {
            self.remember_assistant_response(request, response.response_id, response.assistant)?;
        }
        Ok(())
    }
}

impl ResponseHistory {
    fn get(&mut self, id: &str) -> Option<Vec<Value>> {
        let messages = self.entries.get(id)?.0.clone();
        if let Some(position) = self.order.iter().position(|entry| entry == id) {
            self.order.remove(position);
        }
        self.order.push_back(id.to_owned());
        Some(messages)
    }

    fn put(&mut self, id: String, messages: Vec<Value>) {
        let bytes = serde_json::to_vec(&messages)
            .map(|value| value.len())
            .unwrap_or(MAX_HISTORY_BYTES + 1);
        if bytes > MAX_HISTORY_BYTES {
            return;
        }
        if let Some((_, previous_bytes)) = self.entries.remove(&id) {
            self.total_bytes = self.total_bytes.saturating_sub(previous_bytes);
            if let Some(position) = self.order.iter().position(|entry| entry == &id) {
                self.order.remove(position);
            }
        }
        while self.entries.len() >= MAX_HISTORY_ENTRIES
            || self.total_bytes.saturating_add(bytes) > MAX_HISTORY_BYTES
        {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some((_, removed_bytes)) = self.entries.remove(&oldest) {
                self.total_bytes = self.total_bytes.saturating_sub(removed_bytes);
            }
        }
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        self.order.push_back(id.clone());
        self.entries.insert(id, (messages, bytes));
    }
}

fn validate_provider(provider: &GatewayProvider) -> Result<(), GatewayError> {
    if !valid_id(&provider.id) {
        return Err(GatewayError::InvalidConfig);
    }
    let url = Url::parse(&provider.base_url).map_err(|_| GatewayError::InvalidConfig)?;
    if url.cannot_be_a_base()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(GatewayError::InvalidConfig);
    }
    let loopback = match url.host().ok_or(GatewayError::InvalidConfig)? {
        Host::Domain(host) => host.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => address.is_loopback(),
    };
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(GatewayError::InvalidConfig);
    }
    Ok(())
}

fn endpoint_for(provider: &GatewayProvider) -> Result<String, GatewayError> {
    let suffix = match provider.protocol {
        UpstreamProtocol::AnthropicMessages => "messages",
        UpstreamProtocol::OpenAiChatCompletions => "chat/completions",
        UpstreamProtocol::OpenAiResponses => "responses",
    };
    endpoint_with_suffix(provider, suffix)
}

pub fn models_endpoint_for(provider: &GatewayProvider) -> Result<String, GatewayError> {
    endpoint_with_suffix(provider, "models")
}

pub fn namespaced_model_id(provider_id: &str, upstream_model: &str) -> Option<String> {
    (valid_id(provider_id) && valid_id(upstream_model))
        .then(|| format!("{provider_id}:{upstream_model}"))
}

/// Older LicoUp OpenCode/Pi sidecars used shortened Kimi aliases. Keep those
/// persisted configurations routable without advertising them as current
/// provider catalog entries.
fn legacy_generated_alias<'a>(provider_id: &str, upstream_model: &'a str) -> &'a str {
    if provider_id != "kimi" {
        return upstream_model;
    }
    match upstream_model {
        "k3" => "kimi-k3",
        "k2.7-code" => "kimi-k2.7-code",
        "k2.7-code-highspeed" => "kimi-k2.7-code-highspeed",
        "k2.6" => "kimi-k2.6",
        "k2.5" => "kimi-k2.5",
        _ => upstream_model,
    }
}

fn endpoint_with_suffix(provider: &GatewayProvider, suffix: &str) -> Result<String, GatewayError> {
    let url = Url::parse(&provider.base_url).map_err(|_| GatewayError::InvalidConfig)?;
    let mut base = provider.base_url.trim_end_matches('/').to_owned();
    // A bare origin (path "/") gets the conventional `/v1` segment; providers
    // whose base already carries a path prefix such as `/v1` or `/api/gateway`
    // keep their own.
    if url.path() == "/" {
        base.push_str("/v1");
    }
    Ok(format!("{base}/{suffix}"))
}

fn anthropic_to_chat(document: &Value) -> Result<Value, GatewayError> {
    let mut messages = Vec::new();
    if let Some(system) = document.get("system") {
        let text = content_text(system);
        if !text.is_empty() {
            messages.push(json!({"role":"system","content":text}));
        }
    }
    for message in document
        .get("messages")
        .and_then(Value::as_array)
        .ok_or(GatewayError::InvalidRequest)?
    {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .ok_or(GatewayError::InvalidRequest)?;
        let content = message.get("content").ok_or(GatewayError::InvalidRequest)?;
        if let Some(text) = content.as_str() {
            messages.push(json!({"role":role,"content":text}));
            continue;
        }
        let blocks = content.as_array().ok_or(GatewayError::InvalidRequest)?;
        let mut content_parts = Vec::new();
        let mut tool_calls = Vec::new();
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        content_parts.push(json!({"type":"text","text":text}));
                    }
                }
                Some("image") => {
                    if let Some(image) = anthropic_image_to_chat(block) {
                        content_parts.push(image);
                    }
                }
                Some("tool_use") => tool_calls.push(json!({
                    "id": block.get("id"),
                    "type":"function",
                    "function": {
                        "name": block.get("name"),
                        "arguments": serde_json::to_string(block.get("input").unwrap_or(&Value::Null)).unwrap_or_else(|_| "{}".into())
                    }
                })),
                Some("tool_result") => messages.push(json!({
                    "role":"tool",
                    "tool_call_id":block.get("tool_use_id"),
                    "content":content_text(block.get("content").unwrap_or(&Value::Null))
                })),
                _ => {}
            }
        }
        if !content_parts.is_empty() || !tool_calls.is_empty() {
            let content = if content_parts.len() == 1
                && content_parts[0].get("type").and_then(Value::as_str) == Some("text")
            {
                content_parts[0].get("text").cloned().unwrap_or(Value::Null)
            } else {
                Value::Array(content_parts)
            };
            let mut projected = json!({"role":role,"content":content});
            if !tool_calls.is_empty() {
                projected["tool_calls"] = Value::Array(tool_calls);
            }
            messages.push(projected);
        }
    }
    let mut result = chat_document(document, messages, anthropic_tools(document));
    if let Some(stop_sequences) = document.get("stop_sequences") {
        result["stop"] = stop_sequences.clone();
    }
    if let Some(choice) = document.get("tool_choice") {
        if let Some(mapped) = anthropic_tool_choice(choice) {
            result["tool_choice"] = mapped;
        }
        if let Some(disabled) = choice
            .get("disable_parallel_tool_use")
            .and_then(Value::as_bool)
        {
            result["parallel_tool_calls"] = Value::Bool(!disabled);
        }
    }
    Ok(result)
}

fn responses_to_chat(
    document: &Value,
    previous_messages: Option<&[Value]>,
) -> Result<Value, GatewayError> {
    if document
        .pointer("/reasoning/effort")
        .and_then(Value::as_str)
        .is_some_and(|effort| normalize_reasoning_effort(effort).is_none())
    {
        return Err(GatewayError::InvalidRequest);
    }
    let mut messages = Vec::new();
    if let Some(instructions) = document.get("instructions") {
        let text = content_text(instructions);
        if !text.is_empty() {
            messages.push(json!({"role":"system","content":text}));
        }
    }
    messages.extend(previous_messages.into_iter().flatten().cloned());
    match document.get("input").ok_or(GatewayError::InvalidRequest)? {
        Value::String(text) => messages.push(json!({"role":"user","content":text})),
        Value::Array(items) => {
            for item in items {
                match item.get("type").and_then(Value::as_str) {
                    Some("message") | None => {
                        let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
                        messages.push(json!({
                            "role": role,
                            "content": responses_message_content(item.get("content").unwrap_or(&Value::Null))
                        }));
                    }
                    Some("function_call") | Some("custom_tool_call") => {
                        messages.push(json!({
                            "role":"assistant",
                            "content":"",
                            "tool_calls":[{
                                "id":item.get("call_id").or_else(|| item.get("id")),
                                "type":"function",
                                "function":{
                                    "name":item.get("name"),
                                    "arguments":item.get("arguments").or_else(|| item.get("input")).unwrap_or(&json!("{}"))
                                }
                            }]
                        }));
                    }
                    Some("function_call_output") | Some("custom_tool_call_output") => {
                        messages.push(json!({
                            "role":"tool",
                            "tool_call_id":item.get("call_id"),
                            "content":content_text(item.get("output").unwrap_or(&Value::Null))
                        }));
                    }
                    _ => {}
                }
            }
        }
        _ => return Err(GatewayError::InvalidRequest),
    }
    let mut result = chat_document(document, messages, responses_tools(document));
    if let Some(choice) = document.get("tool_choice") {
        if let Some(mapped) = responses_tool_choice(choice) {
            result["tool_choice"] = mapped;
        }
    }
    if let Some(format) = document.pointer("/text/format") {
        if let Some(mapped) = responses_text_format(format) {
            result["response_format"] = mapped;
        }
    }
    Ok(result)
}

fn chat_document(source: &Value, messages: Vec<Value>, tools: Vec<Value>) -> Value {
    let mut result = json!({
        "model":source.get("model"),
        "messages":messages,
        "stream":source.get("stream").and_then(Value::as_bool).unwrap_or(false)
    });
    for key in [
        "max_tokens",
        "temperature",
        "top_p",
        "stop",
        "reasoning_effort",
        "parallel_tool_calls",
    ] {
        if let Some(value) = source.get(key) {
            result[key] = value.clone();
        }
    }
    if source.get("max_output_tokens").is_some() {
        result["max_tokens"] = source["max_output_tokens"].clone();
    }
    if let Some(effort) = source
        .pointer("/reasoning/effort")
        .and_then(Value::as_str)
        .or_else(|| source.get("reasoning_effort").and_then(Value::as_str))
        .and_then(normalize_reasoning_effort)
    {
        result["reasoning_effort"] = Value::String(effort.to_owned());
    }
    if !tools.is_empty() {
        result["tools"] = Value::Array(tools);
    }
    if result["stream"] == Value::Bool(true) {
        result["stream_options"] = json!({"include_usage":true});
    }
    result
}

fn anthropic_image_to_chat(block: &Value) -> Option<Value> {
    let source = block.get("source")?;
    let url = match source.get("type").and_then(Value::as_str)? {
        "base64" => format!(
            "data:{};base64,{}",
            source.get("media_type")?.as_str()?,
            source.get("data")?.as_str()?
        ),
        "url" => source.get("url")?.as_str()?.to_owned(),
        _ => return None,
    };
    Some(json!({"type":"image_url","image_url":{"url":url}}))
}

fn responses_message_content(content: &Value) -> Value {
    let Some(items) = content.as_array() else {
        return Value::String(content_text(content));
    };
    let parts = items
        .iter()
        .filter_map(|item| match item.get("type").and_then(Value::as_str) {
            Some("input_text") | Some("output_text") | Some("text") => item
                .get("text")
                .and_then(Value::as_str)
                .map(|text| json!({"type":"text","text":text})),
            Some("input_image") => item
                .get("image_url")
                .and_then(Value::as_str)
                .map(|url| json!({"type":"image_url","image_url":{"url":url,"detail":item.get("detail").cloned().unwrap_or(json!("auto"))}})),
            _ => None,
        })
        .collect::<Vec<_>>();
    if parts.len() == 1 && parts[0].get("type").and_then(Value::as_str) == Some("text") {
        parts[0].get("text").cloned().unwrap_or(Value::Null)
    } else {
        Value::Array(parts)
    }
}

fn anthropic_tool_choice(choice: &Value) -> Option<Value> {
    match choice.get("type").and_then(Value::as_str)? {
        "auto" => Some(json!("auto")),
        "any" => Some(json!("required")),
        "none" => Some(json!("none")),
        "tool" => Some(json!({"type":"function","function":{"name":choice.get("name")?}})),
        _ => None,
    }
}

fn responses_tool_choice(choice: &Value) -> Option<Value> {
    if choice.is_string() {
        return Some(choice.clone());
    }
    match choice.get("type").and_then(Value::as_str)? {
        "function" => Some(json!({"type":"function","function":{"name":choice.get("name")?}})),
        _ => None,
    }
}

fn responses_text_format(format: &Value) -> Option<Value> {
    match format.get("type").and_then(Value::as_str)? {
        "text" => Some(json!({"type":"text"})),
        "json_object" => Some(json!({"type":"json_object"})),
        "json_schema" => Some(json!({
            "type":"json_schema",
            "json_schema":{
                "name":format.get("name")?,
                "schema":format.get("schema")?,
                "strict":format.get("strict").cloned().unwrap_or(json!(false))
            }
        })),
        _ => None,
    }
}

fn anthropic_tools(document: &Value) -> Vec<Value> {
    document
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| {
            Some(json!({
                "type":"function",
                "function":{
                    "name":tool.get("name")?,
                    "description":tool.get("description").cloned().unwrap_or(json!("")),
                    "parameters":tool.get("input_schema").cloned().unwrap_or(json!({"type":"object"}))
                }
            }))
        })
        .collect()
}

fn responses_tools(document: &Value) -> Vec<Value> {
    document
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| {
            let tool_type = tool.get("type").and_then(Value::as_str)?;
            let name = tool.get("name").and_then(Value::as_str)?;
            let mut description = tool
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if tool_type == "custom" {
                let metadata = stable_json(tool);
                if !description.is_empty() {
                    description.push('\n');
                }
                description.push_str("Codex custom tool metadata: ");
                description.push_str(&metadata);
            }
            Some(json!({
                "type":"function",
                "function":{
                    "name":name,
                    "description":description,
                    "parameters":tool.get("parameters").cloned().unwrap_or(json!({"type":"object"}))
                }
            }))
        })
        .collect()
}

fn chat_to_responses(body: &[u8], model: &str) -> Result<Vec<u8>, GatewayError> {
    let chat: Value =
        serde_json::from_slice(body).map_err(|_| GatewayError::InvalidUpstreamResponse)?;
    let choice = chat
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .ok_or(GatewayError::InvalidUpstreamResponse)?;
    let id = response_id(&chat);
    let mut output = Vec::new();
    if let Some(reasoning) = choice
        .get("reasoning_content")
        .or_else(|| choice.get("reasoning"))
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        output.push(json!({
            "id":format!("rs_{id}"),"type":"reasoning","status":"completed",
            "summary":[{"type":"summary_text","text":reasoning}]
        }));
    }
    if let Some(text) = choice
        .get("content")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        output.push(json!({
            "id":format!("msg_{id}"),"type":"message","status":"completed","role":"assistant",
            "content":[{"type":"output_text","text":text,"annotations":[]}]
        }));
    }
    if let Some(calls) = choice.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            output.push(json!({
                "id":call.get("id"),"type":"function_call","status":"completed",
                "call_id":call.get("id"),"name":call.pointer("/function/name"),
                "arguments":call.pointer("/function/arguments").cloned().unwrap_or(json!("{}"))
            }));
        }
    }
    let usage = responses_usage(chat.get("usage"));
    let finish_reason = chat
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str);
    let (status, incomplete_details) = responses_completion(finish_reason);
    serde_json::to_vec(&json!({
        "id":id,"object":"response","created_at":unix_seconds(),"status":status,
        "error":null,"incomplete_details":incomplete_details,"model":model,"output":output,
        "parallel_tool_calls":true,"usage":usage
    }))
    .map_err(|_| GatewayError::InvalidUpstreamResponse)
}

fn chat_to_anthropic(body: &[u8], model: &str) -> Result<Vec<u8>, GatewayError> {
    let chat: Value =
        serde_json::from_slice(body).map_err(|_| GatewayError::InvalidUpstreamResponse)?;
    let message = chat
        .pointer("/choices/0/message")
        .ok_or(GatewayError::InvalidUpstreamResponse)?;
    let mut content = Vec::new();
    if let Some(text) = message
        .get("content")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        content.push(json!({"type":"text","text":text}));
    }
    let calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for call in &calls {
        let input = call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or(Value::Object(Map::new()));
        content.push(json!({
                "type":"tool_use","id":call.get("id"),"name":call.pointer("/function/name"),"input":input
            }));
    }
    let usage = chat.get("usage").cloned().unwrap_or(json!({}));
    serde_json::to_vec(&json!({
        "id":response_id(&chat),"type":"message","role":"assistant","model":model,
        "content":content,
        "stop_reason":anthropic_stop_reason(chat.pointer("/choices/0/finish_reason").and_then(Value::as_str), !calls.is_empty()),
        "stop_sequence":null,
        "usage":{"input_tokens":usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
                 "output_tokens":usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0)}
    }))
    .map_err(|_| GatewayError::InvalidUpstreamResponse)
}

fn chat_sse_to_responses(body: &[u8], model: &str) -> Result<Vec<u8>, GatewayError> {
    let chunks = parse_chat_sse(body)?;
    let id = chunks
        .iter()
        .find_map(|chunk| chunk.get("id").and_then(Value::as_str))
        .map(|id| format!("resp_{id}"))
        .unwrap_or_else(|| format!("resp_{}", unix_seconds()));
    let mut reasoning = String::new();
    let mut text = String::new();
    let mut tools: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
    for chunk in &chunks {
        let Some(delta) = chunk.pointer("/choices/0/delta") else {
            continue;
        };
        if let Some(part) = delta
            .get("reasoning_content")
            .or_else(|| delta.get("reasoning"))
            .and_then(Value::as_str)
        {
            reasoning.push_str(part);
        }
        if let Some(part) = delta.get("content").and_then(Value::as_str) {
            text.push_str(part);
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let entry = tools
                    .entry(index)
                    .or_insert_with(|| (String::new(), String::new(), String::new()));
                if let Some(value) = call.get("id").and_then(Value::as_str) {
                    entry.0.push_str(value);
                }
                if let Some(value) = call.pointer("/function/name").and_then(Value::as_str) {
                    entry.1.push_str(value);
                }
                if let Some(value) = call.pointer("/function/arguments").and_then(Value::as_str) {
                    entry.2.push_str(value);
                }
            }
        }
    }
    let mut events = Vec::new();
    let mut sequence = 1u64;
    events.push(sse_event(json!({"type":"response.created","sequence_number":sequence,"response":response_shell(&id, model, "in_progress", Vec::new(), Value::Null)})));
    sequence += 1;
    let mut output = Vec::new();
    let mut output_index = 0usize;
    if !reasoning.is_empty() {
        let item =
            json!({"id":format!("rs_{id}"),"type":"reasoning","status":"in_progress","summary":[]});
        events.push(sse_event(json!({"type":"response.output_item.added","sequence_number":sequence,"output_index":output_index,"item":item})));
        sequence += 1;
        events.push(sse_event(json!({"type":"response.reasoning_summary_part.added","sequence_number":sequence,"output_index":output_index,"summary_index":0,"part":{"type":"summary_text","text":""}})));
        sequence += 1;
        events.push(sse_event(json!({"type":"response.reasoning_summary_text.delta","sequence_number":sequence,"output_index":output_index,"summary_index":0,"delta":reasoning})));
        sequence += 1;
        events.push(sse_event(json!({"type":"response.reasoning_summary_text.done","sequence_number":sequence,"output_index":output_index,"summary_index":0,"text":reasoning})));
        sequence += 1;
        let completed = json!({"id":format!("rs_{id}"),"type":"reasoning","status":"completed","summary":[{"type":"summary_text","text":reasoning}]});
        events.push(sse_event(json!({"type":"response.output_item.done","sequence_number":sequence,"output_index":output_index,"item":completed})));
        sequence += 1;
        output.push(completed);
        output_index += 1;
    }
    if !text.is_empty() {
        let item = json!({"id":format!("msg_{id}"),"type":"message","status":"in_progress","role":"assistant","content":[]});
        events.push(sse_event(json!({"type":"response.output_item.added","sequence_number":sequence,"output_index":output_index,"item":item})));
        sequence += 1;
        events.push(sse_event(json!({"type":"response.content_part.added","sequence_number":sequence,"output_index":output_index,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}})));
        sequence += 1;
        events.push(sse_event(json!({"type":"response.output_text.delta","sequence_number":sequence,"output_index":output_index,"content_index":0,"delta":text})));
        sequence += 1;
        events.push(sse_event(json!({"type":"response.output_text.done","sequence_number":sequence,"output_index":output_index,"content_index":0,"text":text})));
        sequence += 1;
        let part = json!({"type":"output_text","text":text,"annotations":[]});
        events.push(sse_event(json!({"type":"response.content_part.done","sequence_number":sequence,"output_index":output_index,"content_index":0,"part":part})));
        sequence += 1;
        let completed = json!({"id":format!("msg_{id}"),"type":"message","status":"completed","role":"assistant","content":[part]});
        events.push(sse_event(json!({"type":"response.output_item.done","sequence_number":sequence,"output_index":output_index,"item":completed})));
        sequence += 1;
        output.push(completed);
        output_index += 1;
    }
    for (_, (call_id, name, arguments)) in tools {
        let started = json!({"id":call_id,"type":"function_call","status":"in_progress","call_id":call_id,"name":name,"arguments":""});
        events.push(sse_event(json!({"type":"response.output_item.added","sequence_number":sequence,"output_index":output_index,"item":started})));
        sequence += 1;
        events.push(sse_event(json!({"type":"response.function_call_arguments.delta","sequence_number":sequence,"output_index":output_index,"delta":arguments})));
        sequence += 1;
        events.push(sse_event(json!({"type":"response.function_call_arguments.done","sequence_number":sequence,"output_index":output_index,"arguments":arguments})));
        sequence += 1;
        let completed = json!({"id":call_id,"type":"function_call","status":"completed","call_id":call_id,"name":name,"arguments":arguments});
        events.push(sse_event(json!({"type":"response.output_item.done","sequence_number":sequence,"output_index":output_index,"item":completed})));
        sequence += 1;
        output.push(completed);
        output_index += 1;
    }
    let usage = chunks
        .last()
        .map(|chunk| responses_usage(chunk.get("usage")))
        .unwrap_or_else(|| responses_usage(None));
    let finish_reason = chunks.iter().rev().find_map(|chunk| {
        chunk
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)
    });
    let (status, incomplete_details) = responses_completion(finish_reason);
    let mut response = response_shell(&id, model, status, output, usage);
    response["incomplete_details"] = incomplete_details;
    let event_type = if status == "completed" {
        "response.completed"
    } else {
        "response.incomplete"
    };
    events.push(sse_event(
        json!({"type":event_type,"sequence_number":sequence,"response":response}),
    ));
    events.push("data: [DONE]\n\n".into());
    Ok(events.concat().into_bytes())
}

fn chat_sse_to_anthropic(body: &[u8], model: &str) -> Result<Vec<u8>, GatewayError> {
    let chunks = parse_chat_sse(body)?;
    let id = chunks
        .iter()
        .find_map(|chunk| chunk.get("id").and_then(Value::as_str))
        .unwrap_or("gateway-message");
    let (_, assistant) = aggregate_chat_sse(body)?;
    let mut events = vec![sse_named(
        "message_start",
        json!({
            "type":"message_start","message":{"id":id,"type":"message","role":"assistant","model":model,"content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}
        }),
    )];
    let text = assistant
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut block_index = 0usize;
    if !text.is_empty() {
        events.push(sse_named("content_block_start", json!({"type":"content_block_start","index":block_index,"content_block":{"type":"text","text":""}})));
        events.push(sse_named("content_block_delta", json!({"type":"content_block_delta","index":block_index,"delta":{"type":"text_delta","text":text}})));
        events.push(sse_named(
            "content_block_stop",
            json!({"type":"content_block_stop","index":block_index}),
        ));
        block_index += 1;
    }
    let calls = assistant
        .get("tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for call in &calls {
        let arguments = call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .unwrap_or("{}");
        events.push(sse_named("content_block_start", json!({
            "type":"content_block_start","index":block_index,
            "content_block":{"type":"tool_use","id":call.get("id"),"name":call.pointer("/function/name"),"input":{}}
        })));
        events.push(sse_named(
            "content_block_delta",
            json!({
                "type":"content_block_delta","index":block_index,
                "delta":{"type":"input_json_delta","partial_json":arguments}
            }),
        ));
        events.push(sse_named(
            "content_block_stop",
            json!({"type":"content_block_stop","index":block_index}),
        ));
        block_index += 1;
    }
    let usage = chunks.last().and_then(|chunk| chunk.get("usage"));
    let output_tokens = usage
        .and_then(|value| value.get("completion_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let finish_reason = chunks.iter().rev().find_map(|chunk| {
        chunk
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)
    });
    let stop_reason = anthropic_stop_reason(finish_reason, !calls.is_empty());
    events.push(sse_named("message_delta", json!({"type":"message_delta","delta":{"stop_reason":stop_reason,"stop_sequence":null},"usage":{"output_tokens":output_tokens}})));
    events.push(sse_named("message_stop", json!({"type":"message_stop"})));
    Ok(events.concat().into_bytes())
}

fn parse_chat_sse(body: &[u8]) -> Result<Vec<Value>, GatewayError> {
    let text = std::str::from_utf8(body).map_err(|_| GatewayError::InvalidUpstreamResponse)?;
    let mut chunks = Vec::new();
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }
        chunks.push(serde_json::from_str(data).map_err(|_| GatewayError::InvalidUpstreamResponse)?);
    }
    Ok(chunks)
}

fn aggregate_chat_sse(body: &[u8]) -> Result<(String, Value), GatewayError> {
    let chunks = parse_chat_sse(body)?;
    let id = chunks
        .iter()
        .find_map(|chunk| chunk.get("id").and_then(Value::as_str))
        .map(|id| format!("resp_{id}"))
        .unwrap_or_else(|| format!("resp_{}", unix_seconds()));
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut tools: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
    for chunk in &chunks {
        let Some(delta) = chunk.pointer("/choices/0/delta") else {
            continue;
        };
        if let Some(value) = delta.get("content").and_then(Value::as_str) {
            content.push_str(value);
        }
        if let Some(value) = delta
            .get("reasoning_content")
            .or_else(|| delta.get("reasoning"))
            .and_then(Value::as_str)
        {
            reasoning.push_str(value);
        }
        for call in delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let entry = tools
                .entry(index)
                .or_insert_with(|| (String::new(), String::new(), String::new()));
            if let Some(value) = call.get("id").and_then(Value::as_str) {
                entry.0.push_str(value);
            }
            if let Some(value) = call.pointer("/function/name").and_then(Value::as_str) {
                entry.1.push_str(value);
            }
            if let Some(value) = call.pointer("/function/arguments").and_then(Value::as_str) {
                entry.2.push_str(value);
            }
        }
    }
    let tool_calls: Vec<Value> = tools
        .into_values()
        .map(|(call_id, name, arguments)| json!({"id":call_id,"type":"function","function":{"name":name,"arguments":arguments}}))
        .collect();
    Ok((
        id,
        json!({"role":"assistant","content":content,"reasoning_content":reasoning,"tool_calls":tool_calls}),
    ))
}

fn normalize_error(status: u16, body: &[u8]) -> GatewayResponse {
    let parsed = serde_json::from_slice::<Value>(body).ok();
    let message = parsed
        .as_ref()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("message"))
        })
        .and_then(Value::as_str)
        .unwrap_or("upstream request failed");
    let message: String = message.chars().take(1024).collect();
    GatewayResponse {
        status,
        content_type: "application/json",
        body: serde_json::to_vec(&json!({"error":{"message":message,"type":"upstream_error","code":status,"param":null}})).unwrap_or_default(),
    }
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

fn content_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                item.as_str().or_else(|| {
                    item.get("text")
                        .or_else(|| item.get("content"))
                        .and_then(Value::as_str)
                })
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other if !other.is_null() => other.to_string(),
        _ => String::new(),
    }
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
    let reasoning = usage
        .and_then(|value| value.pointer("/completion_tokens_details/reasoning_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    json!({
        "input_tokens":input,"input_tokens_details":{"cached_tokens":0},
        "output_tokens":output,"output_tokens_details":{"reasoning_tokens":reasoning},
        "total_tokens":input.saturating_add(output)
    })
}

fn response_shell(id: &str, model: &str, status: &str, output: Vec<Value>, usage: Value) -> Value {
    json!({"id":id,"object":"response","created_at":unix_seconds(),"status":status,"error":null,"incomplete_details":null,"model":model,"output":output,"parallel_tool_calls":true,"usage":usage})
}

fn response_id(chat: &Value) -> String {
    format!(
        "resp_{}",
        chat.get("id").and_then(Value::as_str).unwrap_or("gateway")
    )
}

fn sse_event(value: Value) -> String {
    format!("data: {value}\n\n")
}
fn sse_named(name: &str, value: Value) -> String {
    format!("event: {name}\ndata: {value}\n\n")
}

fn stable_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let ordered: BTreeMap<_, _> = map.iter().collect();
            serde_json::to_string(&ordered).unwrap_or_default()
        }
        _ => value.to_string(),
    }
}

fn normalize_reasoning_effort(value: &str) -> Option<&'static str> {
    match value {
        "ultra" | "max" | "xhigh" => Some("max"),
        "high" | "medium" => Some("high"),
        "low" | "minimum" | "light" => Some("low"),
        "none" => Some("none"),
        _ => None,
    }
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'[' | b']' | b'~')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_can_compile_without_a_static_model_catalog() {
        let gateway = CompiledGateway::compile(GatewayConfig {
            schema_version: 1,
            providers: vec![],
            routes: vec![],
        })
        .unwrap();
        assert!(
            gateway
                .prepare(
                    "/v1/chat/completions",
                    br#"{"model":"kimi:k3","messages":[]}"#
                )
                .is_err()
        );
        let gateway = CompiledGateway::compile(GatewayConfig {
            schema_version: 1,
            providers: vec![GatewayProvider {
                id: "kimi".into(),
                base_url: "https://api.moonshot.cn/v1".into(),
                protocol: UpstreamProtocol::OpenAiChatCompletions,
                credential_provider: LlmApiKeyProvider::Kimi,
                credential_style: CredentialStyle::Bearer,
            }],
            routes: vec![],
        })
        .unwrap();
        let request = gateway
            .prepare(
                "/v1/chat/completions",
                br#"{"model":"kimi:kimi-k3","messages":[]}"#,
            )
            .unwrap();
        assert_eq!(request.upstream_model, "kimi-k3");
        assert_eq!(
            request.endpoint,
            "https://api.moonshot.cn/v1/chat/completions"
        );
    }

    #[test]
    fn provider_model_catalog_endpoint_keeps_the_configured_base_path() {
        let kilo = GatewayProvider {
            id: "kilo".into(),
            base_url: "https://api.kilo.ai/api/gateway".into(),
            protocol: UpstreamProtocol::OpenAiChatCompletions,
            credential_provider: LlmApiKeyProvider::Kilo,
            credential_style: CredentialStyle::Bearer,
        };
        assert_eq!(
            models_endpoint_for(&kilo).unwrap(),
            "https://api.kilo.ai/api/gateway/models"
        );
        let mut bare = kilo;
        bare.base_url = "https://gateway.example".into();
        assert_eq!(
            models_endpoint_for(&bare).unwrap(),
            "https://gateway.example/v1/models"
        );
    }

    #[test]
    fn previously_generated_kimi_aliases_still_route_without_becoming_a_catalog() {
        let gateway = CompiledGateway::compile(GatewayConfig {
            schema_version: 1,
            providers: vec![GatewayProvider {
                id: "kimi".into(),
                base_url: "https://api.moonshot.cn/v1".into(),
                protocol: UpstreamProtocol::OpenAiChatCompletions,
                credential_provider: LlmApiKeyProvider::Kimi,
                credential_style: CredentialStyle::Bearer,
            }],
            routes: Vec::new(),
        })
        .unwrap();
        let request = gateway
            .prepare(
                "/v1/chat/completions",
                br#"{"model":"kimi:k3","messages":[]}"#,
            )
            .unwrap();
        assert_eq!(request.upstream_model, "kimi-k3");
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["model"], "kimi-k3");
    }

    fn gateway() -> CompiledGateway {
        CompiledGateway::compile(GatewayConfig {
            schema_version: 1,
            providers: vec![
                GatewayProvider {
                    id: "kimi-chat".into(),
                    base_url: "https://api.kimi.example/coding/v1".into(),
                    protocol: UpstreamProtocol::OpenAiChatCompletions,
                    credential_provider: LlmApiKeyProvider::Kimi,
                    credential_style: CredentialStyle::Bearer,
                },
                GatewayProvider {
                    id: "deepseek-anthropic".into(),
                    base_url: "https://deepseek.example/v1".into(),
                    protocol: UpstreamProtocol::AnthropicMessages,
                    credential_provider: LlmApiKeyProvider::DeepSeek,
                    credential_style: CredentialStyle::XApiKey,
                },
            ],
            routes: vec![
                ModelRoute {
                    client_protocol: ClientProtocol::OpenAiResponses,
                    requested_model: "k3-256k".into(),
                    provider_id: "kimi-chat".into(),
                    upstream_model: "k3-256k".into(),
                },
                ModelRoute {
                    client_protocol: ClientProtocol::OpenAiChatCompletions,
                    requested_model: "k3-256k".into(),
                    provider_id: "kimi-chat".into(),
                    upstream_model: "k3-256k".into(),
                },
                ModelRoute {
                    client_protocol: ClientProtocol::AnthropicMessages,
                    requested_model: "backend-rigorous".into(),
                    provider_id: "deepseek-anthropic".into(),
                    upstream_model: "deepseek-v4-flash".into(),
                },
                ModelRoute {
                    client_protocol: ClientProtocol::AnthropicMessages,
                    requested_model: "visual".into(),
                    provider_id: "kimi-chat".into(),
                    upstream_model: "k3-256k".into(),
                },
            ],
        })
        .unwrap()
    }

    #[test]
    fn endpoint_for_appends_v1_only_for_bare_origins() {
        let chat = |base_url: &str| GatewayProvider {
            id: "provider".into(),
            base_url: base_url.into(),
            protocol: UpstreamProtocol::OpenAiChatCompletions,
            credential_provider: LlmApiKeyProvider::Kimi,
            credential_style: CredentialStyle::Bearer,
        };
        assert_eq!(
            endpoint_for(&chat("https://api.kilo.ai/api/gateway")).unwrap(),
            "https://api.kilo.ai/api/gateway/chat/completions"
        );
        assert_eq!(
            endpoint_for(&chat("https://api.kilo.ai/api/gateway/")).unwrap(),
            "https://api.kilo.ai/api/gateway/chat/completions"
        );
        assert_eq!(
            endpoint_for(&chat("https://api.moonshot.cn/v1")).unwrap(),
            "https://api.moonshot.cn/v1/chat/completions"
        );
        assert_eq!(
            endpoint_for(&chat("https://api.moonshot.cn")).unwrap(),
            "https://api.moonshot.cn/v1/chat/completions"
        );
        assert_eq!(
            endpoint_for(&chat("https://api.moonshot.cn/")).unwrap(),
            "https://api.moonshot.cn/v1/chat/completions"
        );
    }

    #[test]
    fn exact_model_routes_select_provider_without_active_global_switch() {
        let gateway = gateway();
        let codex = gateway
            .prepare(
                "/v1/responses",
                serde_json::to_string(&json!({
                    "model":"k3-256k","input":"hello","stream":true,
                    "reasoning":{"effort":"xhigh"},
                    "tools":[{"type":"function","name":"read_file","description":"read","parameters":{"type":"object"}}]
                }))
                .unwrap()
                .as_bytes(),
            )
            .unwrap();
        assert_eq!(
            codex.upstream_protocol,
            UpstreamProtocol::OpenAiChatCompletions
        );
        assert!(codex.endpoint.ends_with("/v1/chat/completions"));
        let body: Value = serde_json::from_slice(&codex.body).unwrap();
        assert_eq!(body["messages"][0]["content"], "hello");
        assert_eq!(body["reasoning_effort"], "max");
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");

        let claude = gateway
            .prepare(
                "/v1/messages",
                br#"{"model":"backend-rigorous","messages":[{"role":"user","content":"hello"}]}"#,
            )
            .unwrap();
        assert_eq!(
            claude.upstream_protocol,
            UpstreamProtocol::AnthropicMessages
        );
        assert_eq!(claude.upstream_model, "deepseek-v4-flash");
    }

    #[test]
    fn converted_requests_preserve_multimodal_tools_stops_and_structured_output() {
        let gateway = gateway();
        let anthropic = gateway
            .prepare(
                "/v1/messages",
                serde_json::to_vec(&json!({
                    "model":"visual","max_tokens":512,"stream":true,
                    "stop_sequences":["END"],
                    "tool_choice":{"type":"tool","name":"browser","disable_parallel_tool_use":true},
                    "tools":[{"name":"browser","input_schema":{"type":"object"}}],
                    "messages":[{"role":"user","content":[
                        {"type":"text","text":"inspect"},
                        {"type":"image","source":{"type":"url","url":"https://example.invalid/image.png"}}
                    ]}]
                }))
                .unwrap()
                .as_slice(),
            )
            .unwrap();
        let body: Value = serde_json::from_slice(&anthropic.body).unwrap();
        assert_eq!(body["stop"], json!(["END"]));
        assert_eq!(body["parallel_tool_calls"], false);
        assert_eq!(body["tool_choice"]["function"]["name"], "browser");
        assert_eq!(
            body["messages"][0]["content"][1]["image_url"]["url"],
            "https://example.invalid/image.png"
        );
        assert_eq!(body["stream_options"]["include_usage"], true);

        let responses = gateway
            .prepare(
                "/v1/responses",
                serde_json::to_vec(&json!({
                    "model":"k3-256k","parallel_tool_calls":false,
                    "tool_choice":{"type":"function","name":"read_file"},
                    "tools":[{"type":"function","name":"read_file","parameters":{"type":"object"}}],
                    "text":{"format":{"type":"json_schema","name":"answer","schema":{"type":"object"},"strict":true}},
                    "input":[{"type":"message","role":"user","content":[
                        {"type":"input_text","text":"inspect"},
                        {"type":"input_image","image_url":"https://example.invalid/image.png","detail":"high"}
                    ]}]
                }))
                .unwrap()
                .as_slice(),
            )
            .unwrap();
        let body: Value = serde_json::from_slice(&responses.body).unwrap();
        assert_eq!(body["parallel_tool_calls"], false);
        assert_eq!(body["tool_choice"]["function"]["name"], "read_file");
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(
            body["messages"][0]["content"][1]["image_url"]["detail"],
            "high"
        );
    }

    #[test]
    fn chat_json_is_rebuilt_as_strict_responses_shape_with_reasoning_usage() {
        let gateway = gateway();
        let request = gateway
            .prepare("/v1/responses", br#"{"model":"k3-256k","input":"hello"}"#)
            .unwrap();
        let response = gateway
            .finish(
                &request,
                200,
                Some("application/json"),
                br#"{"id":"chat-1","choices":[{"message":{"content":"done","tool_calls":[]}}],"usage":{"prompt_tokens":3,"completion_tokens":2}}"#,
            )
            .unwrap();
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["object"], "response");
        assert_eq!(body["output"][0]["content"][0]["text"], "done");
        assert_eq!(
            body["usage"]["output_tokens_details"]["reasoning_tokens"],
            0
        );
    }

    #[test]
    fn converted_responses_preserve_incomplete_and_anthropic_stop_semantics() {
        let gateway = gateway();
        let responses_request = gateway
            .prepare("/v1/responses", br#"{"model":"k3-256k","input":"hello"}"#)
            .unwrap();
        let response = gateway
            .finish(
                &responses_request,
                200,
                Some("application/json"),
                br#"{"id":"chat-1","choices":[{"message":{"content":"partial"},"finish_reason":"length"}]}"#,
            )
            .unwrap();
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["status"], "incomplete");
        assert_eq!(body["incomplete_details"]["reason"], "max_output_tokens");

        let anthropic_request = gateway
            .prepare(
                "/v1/messages",
                br#"{"model":"visual","max_tokens":32,"messages":[{"role":"user","content":"hello"}]}"#,
            )
            .unwrap();
        let response = gateway
            .finish(
                &anthropic_request,
                200,
                Some("application/json"),
                br#"{"id":"chat-2","choices":[{"message":{"content":"partial"},"finish_reason":"length"}]}"#,
            )
            .unwrap();
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["stop_reason"], "max_tokens");
    }

    #[test]
    fn openai_chat_completions_json_and_stream_are_passed_through() {
        let gateway = gateway();
        let request = gateway
            .prepare(
                "/v1/chat/completions",
                br#"{"model":"k3-256k","messages":[{"role":"user","content":"hello"}]}"#,
            )
            .unwrap();
        assert_eq!(
            request.client_protocol,
            ClientProtocol::OpenAiChatCompletions
        );
        assert_eq!(
            request.upstream_protocol,
            UpstreamProtocol::OpenAiChatCompletions
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&request.body).unwrap()["messages"][0]["content"],
            "hello"
        );

        let json =
            br#"{"id":"chat-1","choices":[{"message":{"role":"assistant","content":"done"}}]}"#;
        let response = gateway
            .finish(&request, 200, Some("application/json"), json)
            .unwrap();
        assert_eq!(response.content_type, "application/json");
        assert_eq!(response.body, json);

        let streaming = gateway
            .prepare(
                "/chat/completions",
                br#"{"model":"k3-256k","stream":true,"messages":[]}"#,
            )
            .unwrap();
        let sse = b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n";
        let response = gateway
            .finish(&streaming, 200, Some("text/event-stream"), sse)
            .unwrap();
        assert_eq!(response.content_type, "text/event-stream");
        assert_eq!(response.body, sse);
    }

    #[test]
    fn chat_sse_is_rebuilt_as_responses_events_and_tools() {
        let gateway = gateway();
        let request = gateway
            .prepare(
                "/v1/responses",
                br#"{"model":"k3-256k","input":"hello","stream":true}"#,
            )
            .unwrap();
        let upstream = concat!(
            "data: {\"id\":\"chat-1\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: {\"id\":\"chat-1\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{}\"}}]}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let response = gateway
            .finish(
                &request,
                200,
                Some("text/event-stream"),
                upstream.as_bytes(),
            )
            .unwrap();
        let text = String::from_utf8(response.body).unwrap();
        assert!(text.contains("response.created"));
        assert!(text.contains("response.output_item.added"));
        assert!(text.contains("response.output_text.delta"));
        assert!(text.contains("response.function_call_arguments.delta"));
        assert!(text.contains("response.completed"));
    }

    #[test]
    fn previous_response_id_restores_bounded_chat_history() {
        let gateway = gateway();
        let first = gateway
            .prepare(
                "/v1/responses",
                br#"{"model":"k3-256k","instructions":"one-turn rule","input":"first"}"#,
            )
            .unwrap();
        gateway
            .finish(
                &first,
                200,
                Some("application/json"),
                br#"{"id":"chat-1","choices":[{"message":{"role":"assistant","content":"answer"}}]}"#,
            )
            .unwrap();
        let second = gateway
            .prepare(
                "/v1/responses",
                br#"{"model":"k3-256k","input":"second","previous_response_id":"resp_chat-1"}"#,
            )
            .unwrap();
        let body: Value = serde_json::from_slice(&second.body).unwrap();
        assert_eq!(body["messages"][0]["content"], "first");
        assert_eq!(body["messages"][1]["content"], "answer");
        assert_eq!(body["messages"][2]["content"], "second");
        assert!(
            body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|message| message["role"] != "system")
        );
    }

    #[test]
    fn claude_gateway_rebuilds_chat_tool_stream_as_anthropic_events() {
        let gateway = gateway();
        let request = gateway
            .prepare(
                "/v1/messages",
                br#"{"model":"visual","stream":true,"messages":[{"role":"user","content":"inspect"}],"tools":[{"name":"browser","input_schema":{"type":"object"}}]}"#,
            )
            .unwrap();
        let upstream = concat!(
            "data: {\"id\":\"chat-tool\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"tool-1\",\"function\":{\"name\":\"browser\",\"arguments\":\"{\\\"url\\\":\\\"https://example.invalid\\\"}\"}}]}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let response = gateway
            .finish(
                &request,
                200,
                Some("text/event-stream"),
                upstream.as_bytes(),
            )
            .unwrap();
        let text = String::from_utf8(response.body).unwrap();
        assert!(text.contains("content_block_start"));
        assert!(text.contains("tool_use"));
        assert!(text.contains("input_json_delta"));
        assert!(text.contains("message_stop"));
    }

    #[test]
    fn configuration_and_unknown_routes_fail_closed() {
        let gateway = gateway();
        assert_eq!(
            gateway.prepare("/v1/messages", br#"{"model":"unknown","messages":[]}"#),
            Err(GatewayError::RouteNotFound)
        );
        assert_eq!(
            gateway.prepare(
                "/v1/chat/completions",
                br#"{"model":"unknown-provider:model","messages":[]}"#
            ),
            Err(GatewayError::RouteNotFound)
        );
        assert_eq!(
            gateway.prepare(
                "/v1/chat/completions",
                br#"{"model":"kimi-chat:","messages":[]}"#
            ),
            Err(GatewayError::RouteNotFound)
        );
        assert_eq!(
            gateway.prepare(
                "/v1/responses",
                br#"{"model":"k3-256k","input":"x","previous_response_id":"resp-old"}"#
            ),
            Err(GatewayError::UnsupportedConversion)
        );
        assert_eq!(
            gateway.prepare(
                "/v1/responses",
                br#"{"model":"k3-256k","input":"x","reasoning":{"effort":"invented"}}"#
            ),
            Err(GatewayError::InvalidRequest)
        );
    }
}
