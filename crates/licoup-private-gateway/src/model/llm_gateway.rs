//! Closed model routing and client-dialect projection for the local LLM gateway.
//!
//! This module is the host shell's half of the forward path. It resolves which
//! provider lane a wire request rides, decodes the client dialect into a4mp's
//! canonical `ChatRequest`, and renders canonical outputs back into the client
//! dialect. The provider-facing conversion, adapter dispatch and upstream
//! response decoding live in fairliny/a4mp — `PreparedGatewayRequest` is the
//! hand-off document between the two halves, and `fairliny_gateway::router` is
//! where the canonical request is dispatched from.
//!
//! Configuration contains credential references only. Secrets are resolved by
//! the host's lease port after routing and endpoint validation succeed.

use crate::credential::LlmApiKeyProvider;
use a4mp::wire::{self, WireDialect};
use a4mp::{
    AuthKind, ChatRequest, ContentPart, Message, Output, Protocol, ProviderId, Role, SseEvent,
    ToolDef,
};
use fairliny_gateway::router::Route;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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

/// A routed, canonically decoded request plus the fairliny route binding that
/// dispatches it. The canonical request already carries the upstream model id;
/// `route` binds provider, protocol, auth mechanism and the materialized
/// endpoint for fairliny's router.
#[derive(Clone, Debug)]
pub struct PreparedGatewayRequest {
    pub client_protocol: ClientProtocol,
    pub upstream_protocol: UpstreamProtocol,
    pub credential_provider: LlmApiKeyProvider,
    pub(crate) route: Route,
    pub request: ChatRequest,
    pub stream: bool,
    pub requested_model: String,
    pub upstream_model: String,
    /// Request messages captured for history replay — set only for Responses
    /// clients on non-Responses upstreams, where `previous_response_id`
    /// continuation is the shell's job.
    pub(crate) history_messages: Option<Vec<Message>>,
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
    profiles: Vec<a4mp::ProviderProfile>,
    credential_kinds: HashMap<String, LlmApiKeyProvider>,
    history: Mutex<ResponseHistory>,
}

#[derive(Default)]
struct ResponseHistory {
    entries: HashMap<String, (Vec<Message>, usize)>,
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
        let mut profiles = Vec::with_capacity(config.providers.len());
        let mut credential_kinds = HashMap::with_capacity(config.providers.len());
        for provider in config.providers {
            validate_provider(&provider)?;
            credential_kinds.insert(provider.id.clone(), provider.credential_provider);
            profiles.push(provider_profile(&provider));
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
            profiles,
            credential_kinds,
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

    /// a4mp provider profiles fairliny binds adapters against — one per
    /// configured lane, endpoints always materialized through the `{endpoint}`
    /// variable so no address is baked into a profile.
    pub(crate) fn provider_profiles(&self) -> &[a4mp::ProviderProfile] {
        &self.profiles
    }

    /// Provider id → lease kind, for the credential resolver port.
    pub(crate) fn credential_kinds(&self) -> &HashMap<String, LlmApiKeyProvider> {
        &self.credential_kinds
    }

    /// The fairliny route binding for one provider lane: the endpoint rides
    /// the `{endpoint}` variable, the auth mechanism mirrors the configured
    /// credential style.
    pub(crate) fn provider_route(
        &self,
        provider: &GatewayProvider,
        upstream_model: &str,
    ) -> Result<Route, GatewayError> {
        Ok(Route {
            provider: ProviderId::new(provider.id.clone()),
            protocol: Some(a4mp_protocol(provider.protocol)),
            region: None,
            auth: auth_kind(provider.credential_style),
            upstream_model: upstream_model.to_owned(),
            vars: BTreeMap::from([("endpoint".to_owned(), endpoint_base(provider)?)]),
        })
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
        let document: Value =
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
        let (mut chat, _report) =
            wire::decode_chat_request(client_wire_dialect(client_protocol), body)
                .map_err(|_| GatewayError::InvalidRequest)?;

        // Responses continuation is the shell's concept — the protocol layer
        // has none — so the history splice happens here before the canonical
        // request is handed downstream. A same-dialect Responses upstream keeps
        // the field: it lands in `extensions` and the adapter forwards it, so
        // the provider's own continuation still answers.
        let mut history_messages = None;
        if client_protocol == ClientProtocol::OpenAiResponses {
            if provider.protocol != UpstreamProtocol::OpenAiResponses {
                match chat.extensions.get("previous_response_id") {
                    None | Some(Value::Null) => {}
                    Some(Value::String(id)) if valid_id(id) => {
                        let mut previous = self
                            .history
                            .lock()
                            .map_err(|_| GatewayError::InvalidRequest)?
                            .get(id)
                            .ok_or(GatewayError::UnsupportedConversion)?;
                        // Reasoning state is origin-bound: replay only what the
                        // target protocol itself issued — a foreign block would
                        // refuse at encode time.
                        let upstream = a4mp_protocol(provider.protocol);
                        for message in &mut previous {
                            message.content.retain(|part| {
                                !matches!(part, ContentPart::Reasoning(block)
                                    if block.protocol != upstream)
                            });
                        }
                        let insert_at = chat
                            .messages
                            .iter()
                            .position(|message| message.role != Role::System)
                            .unwrap_or(chat.messages.len());
                        chat.messages.splice(insert_at..insert_at, previous);
                    }
                    _ => return Err(GatewayError::InvalidRequest),
                }
                chat.extensions.remove("previous_response_id");
            }
            // Custom tools have no canonical home; for a foreign upstream their
            // metadata folds into function definitions, for a same-dialect
            // upstream the verbatim array overrides the canonical list at
            // build time.
            if provider.protocol == UpstreamProtocol::OpenAiResponses {
                if let Some(tools) = document.get("tools") {
                    chat.extensions.insert("tools".into(), tools.clone());
                }
            } else {
                chat.tools.extend(custom_tool_defs(&document));
            }
        }
        match provider.protocol {
            UpstreamProtocol::OpenAiChatCompletions => {
                // The Responses dialect speaks `reasoning.effort`; Chat
                // upstreams speak `reasoning_effort`. Bridging dialect-owned
                // fields is the shell's job — they are not canonical.
                let effort = chat
                    .extensions
                    .get("reasoning")
                    .and_then(|value| value.get("effort"))
                    .and_then(Value::as_str)
                    .or_else(|| {
                        chat.extensions
                            .get("reasoning_effort")
                            .and_then(Value::as_str)
                    })
                    .map(str::to_owned);
                if let Some(effort) = effort {
                    let normalized =
                        normalize_reasoning_effort(&effort).ok_or(GatewayError::InvalidRequest)?;
                    chat.extensions.insert(
                        "reasoning_effort".into(),
                        Value::String(normalized.to_owned()),
                    );
                }
                chat.extensions.remove("reasoning");
                if client_protocol == ClientProtocol::OpenAiResponses {
                    // `text.format` is the Responses spelling of Chat's
                    // `response_format`.
                    if let Some(format) = chat
                        .extensions
                        .get("text")
                        .and_then(|text| text.get("format"))
                        && let Some(mapped) = responses_text_format(format)
                    {
                        chat.extensions.insert("response_format".into(), mapped);
                    }
                    chat.extensions.remove("text");
                }
                if stream {
                    chat.extensions
                        .entry("stream_options")
                        .or_insert_with(|| json!({"include_usage": true}));
                }
            }
            UpstreamProtocol::AnthropicMessages => {
                for key in [
                    "reasoning",
                    "reasoning_effort",
                    "text",
                    "response_format",
                    "stream_options",
                ] {
                    chat.extensions.remove(key);
                }
            }
            UpstreamProtocol::OpenAiResponses => {}
        }
        if client_protocol == ClientProtocol::OpenAiResponses
            && provider.protocol != UpstreamProtocol::OpenAiResponses
        {
            history_messages = Some(chat.messages.clone());
        }
        chat.model = route.upstream_model.clone();
        let upstream_model = route.upstream_model.clone();
        let route = self.provider_route(provider, &upstream_model)?;
        Ok(PreparedGatewayRequest {
            client_protocol,
            upstream_protocol: provider.protocol,
            credential_provider: provider.credential_provider,
            route,
            request: chat,
            stream,
            requested_model,
            upstream_model,
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

    /// Canonical output → client dialect JSON. For Responses clients on
    /// non-Responses upstreams the turn is also recorded under the emitted
    /// response id so a `previous_response_id` follow-up can replay it.
    pub fn finish(
        &self,
        request: &PreparedGatewayRequest,
        output: &Output,
    ) -> Result<GatewayResponse, GatewayError> {
        let (value, _report) = wire::encode_chat_output(
            client_wire_dialect(request.client_protocol),
            output,
            &request.requested_model,
        );
        if request.client_protocol == ClientProtocol::OpenAiResponses
            && request.upstream_protocol != UpstreamProtocol::OpenAiResponses
        {
            let response_id = value
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("resp_{}", unix_seconds()));
            self.remember_assistant_response(request, response_id, assistant_message(output))?;
        }
        let body = serde_json::to_vec(&value).map_err(|_| GatewayError::InvalidUpstreamResponse)?;
        Ok(GatewayResponse {
            status: 200,
            content_type: "application/json",
            body,
        })
    }

    fn remember_assistant_response(
        &self,
        request: &PreparedGatewayRequest,
        response_id: String,
        assistant: Message,
    ) -> Result<(), GatewayError> {
        let mut messages = request.history_messages.clone().unwrap_or_default();
        messages.retain(|message| message.role != Role::System);
        messages.push(assistant);
        self.history
            .lock()
            .map_err(|_| GatewayError::InvalidUpstreamResponse)?
            .put(response_id, messages);
        Ok(())
    }

    /// `prepare` accepts `previous_response_id` from a Responses client
    /// whenever the upstream is not itself Responses; the streamed path must
    /// store the same history the buffered path stores, or a second streamed
    /// turn is refused on a transport difference rather than a protocol one.
    pub fn remember_stream_response(
        &self,
        request: &PreparedGatewayRequest,
        response: crate::model::llm_gateway_stream::StreamedChatResponse,
    ) -> Result<(), GatewayError> {
        if request.client_protocol == ClientProtocol::OpenAiResponses
            && request.upstream_protocol != UpstreamProtocol::OpenAiResponses
        {
            self.remember_assistant_response(request, response.response_id, response.assistant)?;
        }
        Ok(())
    }
}

impl ResponseHistory {
    fn get(&mut self, id: &str) -> Option<Vec<Message>> {
        let messages = self.entries.get(id)?.0.clone();
        if let Some(position) = self.order.iter().position(|entry| entry == id) {
            self.order.remove(position);
        }
        self.order.push_back(id.to_owned());
        Some(messages)
    }

    fn put(&mut self, id: String, messages: Vec<Message>) {
        let bytes = messages_bytes(&messages);
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

/// One provider lane as an a4mp profile. The endpoint is never stored: the
/// `{endpoint}` variable carries it per bind so the profile stays a pure
/// capability declaration.
fn provider_profile(provider: &GatewayProvider) -> a4mp::ProviderProfile {
    a4mp::ProviderProfile {
        id: ProviderId::new(provider.id.clone()),
        name: "licoup",
        aliases: &[],
        auth: vec![a4mp::AuthSpec::ApiKey, a4mp::AuthSpec::Bearer],
        targets: vec![a4mp::ServiceDef {
            protocol: a4mp_protocol(provider.protocol),
            base_url: "{endpoint}",
            region: None,
            headers: &[],
        }],
        catalogue: None,
    }
}

fn a4mp_protocol(protocol: UpstreamProtocol) -> Protocol {
    match protocol {
        UpstreamProtocol::AnthropicMessages => Protocol::AnthropicMessages,
        UpstreamProtocol::OpenAiChatCompletions => Protocol::OpenAiChatCompletions,
        UpstreamProtocol::OpenAiResponses => Protocol::OpenAiResponses,
    }
}

fn auth_kind(style: CredentialStyle) -> AuthKind {
    match style {
        CredentialStyle::Bearer => AuthKind::Bearer,
        CredentialStyle::XApiKey => AuthKind::ApiKey,
    }
}

/// The client dialects a4mp decodes and encodes — identical vocabulary to
/// `ClientProtocol`, kept as separate enums because the route table treats
/// them asymmetrically.
pub(crate) fn client_wire_dialect(client: ClientProtocol) -> WireDialect {
    match client {
        ClientProtocol::AnthropicMessages => WireDialect::AnthropicMessages,
        ClientProtocol::OpenAiChatCompletions => WireDialect::OpenAiChatCompletions,
        ClientProtocol::OpenAiResponses => WireDialect::OpenAiResponses,
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

/// The base URL the provider lane's adapter joins API paths onto. Chat and
/// Responses upstreams get the conventional `/v1` segment appended to bare
/// origins; the Messages adapter prefixes `v1/` itself, so a configured `/v1`
/// suffix would double it and is stripped.
pub(crate) fn endpoint_base(provider: &GatewayProvider) -> Result<String, GatewayError> {
    let url = Url::parse(&provider.base_url).map_err(|_| GatewayError::InvalidConfig)?;
    let mut base = provider.base_url.trim_end_matches('/').to_owned();
    match provider.protocol {
        UpstreamProtocol::AnthropicMessages => {
            if let Some(stripped) = base.strip_suffix("/v1") {
                base = stripped.to_owned();
            }
        }
        _ => {
            if url.path() == "/" {
                base.push_str("/v1");
            }
        }
    }
    Ok(base)
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

fn assistant_message(output: &Output) -> Message {
    Message {
        role: Role::Assistant,
        content: output.parts.clone(),
    }
}

/// Responses `custom` tools (codex apply_patch and kin) have no canonical
/// home; their metadata folds into a function definition so the capability
/// survives a foreign upstream instead of being dropped.
fn custom_tool_defs(document: &Value) -> Vec<ToolDef> {
    document
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|tool| tool.get("type").and_then(Value::as_str) == Some("custom"))
        .filter_map(|tool| {
            let name = tool.get("name").and_then(Value::as_str)?;
            let mut description = tool
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let metadata = stable_json(tool);
            if !description.is_empty() {
                description.push('\n');
            }
            description.push_str("Codex custom tool metadata: ");
            description.push_str(&metadata);
            Some(ToolDef {
                name: name.to_owned(),
                description,
                parameters: tool
                    .get("parameters")
                    .cloned()
                    .unwrap_or_else(|| json!({"type":"object"})),
                strict: false,
            })
        })
        .collect()
}

/// `text.format` (Responses) → `response_format` (Chat).
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

pub(crate) fn sse_frame(frame: &SseEvent) -> Vec<u8> {
    let mut text = String::new();
    if let Some(event) = &frame.event {
        text.push_str("event: ");
        text.push_str(event);
        text.push('\n');
    }
    for line in frame.data.lines() {
        text.push_str("data: ");
        text.push_str(line);
        text.push('\n');
    }
    text.push('\n');
    text.into_bytes()
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

/// Bounded byte estimate for a canonical message list: covers text, reasoning
/// text and tool-call payloads so the history cap tracks serialized size
/// closely enough without a second serialization pass.
fn messages_bytes(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|message| {
            64 + message
                .content
                .iter()
                .map(|part| match part {
                    ContentPart::Text { text } => text.len(),
                    ContentPart::ToolCall(call) => {
                        call.id.len() + call.name.len() + call.arguments.to_string().len()
                    }
                    ContentPart::ToolResult(result) => {
                        result.call_id.len()
                            + result
                                .content
                                .iter()
                                .map(|part| match part {
                                    ContentPart::Text { text } => text.len(),
                                    _ => 64,
                                })
                                .sum::<usize>()
                    }
                    ContentPart::Reasoning(block) => {
                        block.opaque.to_string().len()
                            + block.summary.as_deref().map(str::len).unwrap_or(0)
                    }
                    _ => 64,
                })
                .sum::<usize>()
        })
        .sum()
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
    use a4mp::{ReasoningBlock, StopReason, Usage};

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
        assert_eq!(request.route.vars["endpoint"], "https://api.moonshot.cn/v1");
        assert!(matches!(request.route.auth, AuthKind::Bearer));
    }

    #[test]
    fn endpoint_base_appends_v1_only_where_the_adapter_does_not() {
        let provider = |base_url: &str, protocol: UpstreamProtocol| GatewayProvider {
            id: "provider".into(),
            base_url: base_url.into(),
            protocol,
            credential_provider: LlmApiKeyProvider::Kimi,
            credential_style: CredentialStyle::Bearer,
        };
        assert_eq!(
            endpoint_base(&provider(
                "https://api.kilo.ai/api/gateway",
                UpstreamProtocol::OpenAiChatCompletions
            ))
            .unwrap(),
            "https://api.kilo.ai/api/gateway"
        );
        assert_eq!(
            endpoint_base(&provider(
                "https://api.moonshot.cn",
                UpstreamProtocol::OpenAiChatCompletions
            ))
            .unwrap(),
            "https://api.moonshot.cn/v1"
        );
        assert_eq!(
            endpoint_base(&provider(
                "https://api.moonshot.cn/",
                UpstreamProtocol::OpenAiResponses
            ))
            .unwrap(),
            "https://api.moonshot.cn/v1"
        );
        // The Messages adapter prefixes `v1/` itself — a configured suffix
        // would double it.
        assert_eq!(
            endpoint_base(&provider(
                "https://api.anthropic.com/v1",
                UpstreamProtocol::AnthropicMessages
            ))
            .unwrap(),
            "https://api.anthropic.com"
        );
        assert_eq!(
            endpoint_base(&provider(
                "https://api.anthropic.com",
                UpstreamProtocol::AnthropicMessages
            ))
            .unwrap(),
            "https://api.anthropic.com"
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
        assert_eq!(request.request.model, "kimi-k3");
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
                ModelRoute {
                    client_protocol: ClientProtocol::OpenAiChatCompletions,
                    requested_model: "chat-on-anthropic".into(),
                    provider_id: "deepseek-anthropic".into(),
                    upstream_model: "deepseek-v4-flash".into(),
                },
                ModelRoute {
                    client_protocol: ClientProtocol::OpenAiResponses,
                    requested_model: "responses-on-anthropic".into(),
                    provider_id: "deepseek-anthropic".into(),
                    upstream_model: "deepseek-v4-flash".into(),
                },
            ],
        })
        .unwrap()
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
        assert_eq!(
            codex.route.vars["endpoint"],
            "https://api.kimi.example/coding/v1"
        );
        assert!(matches!(codex.route.auth, AuthKind::Bearer));
        assert!(codex.stream);
        assert_eq!(codex.request.model, "k3-256k");
        assert_eq!(codex.request.extensions["reasoning_effort"], json!("max"));
        assert_eq!(
            codex.request.extensions["stream_options"],
            json!({"include_usage": true})
        );
        assert_eq!(codex.request.tools[0].name, "read_file");
        assert!(matches!(
            codex.request.messages[0].content[0],
            ContentPart::Text { ref text } if text == "hello"
        ));

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
        assert!(matches!(claude.route.auth, AuthKind::ApiKey));
        assert_eq!(claude.route.vars["endpoint"], "https://deepseek.example");
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
        assert_eq!(anthropic.request.sampling.stop, vec!["END".to_owned()]);
        assert_eq!(anthropic.request.parallel_tool_calls, Some(false));
        assert_eq!(
            anthropic.request.tool_choice,
            Some(a4mp::ToolChoice::Named("browser".to_owned()))
        );
        assert_eq!(
            anthropic.request.extensions["stream_options"],
            json!({"include_usage": true})
        );
        assert!(
            anthropic.request.messages[0]
                .content
                .iter()
                .any(|part| matches!(part, ContentPart::Media { .. }))
        );

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
        assert_eq!(responses.request.parallel_tool_calls, Some(false));
        assert_eq!(
            responses.request.tool_choice,
            Some(a4mp::ToolChoice::Named("read_file".to_owned()))
        );
        assert_eq!(
            responses.request.extensions["response_format"],
            json!({"type":"json_schema","json_schema":{"name":"answer","schema":{"type":"object"},"strict":true}})
        );
        assert!(
            responses.request.messages[0]
                .content
                .iter()
                .any(|part| matches!(part, ContentPart::Media { .. }))
        );
    }

    #[test]
    fn chat_finish_is_encoded_as_the_client_responses_shape() {
        let gateway = gateway();
        let request = gateway
            .prepare("/v1/responses", br#"{"model":"k3-256k","input":"hello"}"#)
            .unwrap();
        let response = gateway
            .finish(
                &request,
                &Output {
                    parts: vec![ContentPart::Text {
                        text: "done".to_owned(),
                    }],
                    stop: Some(StopReason::Complete),
                    usage: Some(Usage {
                        input_tokens: Some(3),
                        output_tokens: Some(2),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .unwrap();
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["object"], "response");
        assert_eq!(body["output"][0]["content"][0]["text"], "done");
        assert_eq!(body["usage"]["output_tokens"], 2);
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
                &Output {
                    parts: vec![ContentPart::Text {
                        text: "partial".to_owned(),
                    }],
                    stop: Some(StopReason::Length),
                    ..Default::default()
                },
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
                &Output {
                    parts: vec![ContentPart::Text {
                        text: "partial".to_owned(),
                    }],
                    stop: Some(StopReason::Length),
                    ..Default::default()
                },
            )
            .unwrap();
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["stop_reason"], "max_tokens");
    }

    /// A Chat-Completions client routed onto an Anthropic upstream crosses the
    /// canonical request/response boundary in both directions.
    #[test]
    fn chat_client_on_anthropic_upstream_round_trips() {
        let gateway = gateway();
        let request = gateway
            .prepare(
                "/v1/chat/completions",
                br#"{"model":"chat-on-anthropic","messages":[{"role":"user","content":"hello"}],"max_tokens":16}"#,
            )
            .unwrap();
        assert_eq!(
            request.upstream_protocol,
            UpstreamProtocol::AnthropicMessages
        );
        assert_eq!(request.upstream_model, "deepseek-v4-flash");
        assert_eq!(request.request.model, "deepseek-v4-flash");
        assert_eq!(request.request.sampling.max_output_tokens, Some(16));

        let output = a4mp::protocols::anthropic_messages::decode_response(
            br#"{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"done"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":2}}"#,
        )
        .unwrap();
        let response = gateway.finish(&request, &output).unwrap();
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["choices"][0]["message"]["content"], "done");
        assert_eq!(body["usage"]["prompt_tokens"], 3);
        assert_eq!(body["usage"]["completion_tokens"], 2);
    }

    /// The same cross-dialect relay carries a live stream: upstream Anthropic
    /// SSE is decoded to canonical events upstream of the sink and re-encoded
    /// for the Chat client by the shell's stream relay.
    #[test]
    fn chat_client_on_anthropic_upstream_streams() {
        let gateway = gateway();
        let request = gateway
            .prepare(
                "/v1/chat/completions",
                br#"{"model":"chat-on-anthropic","stream":true,"messages":[{"role":"user","content":"hi"}],"max_tokens":16}"#,
            )
            .unwrap();
        let mut relay = crate::model::llm_gateway_stream::ClientStreamRelay::new(&request);
        let mut text = relay.preamble();
        for event in [
            a4mp::StreamEvent::PartStarted {
                index: 0,
                kind: a4mp::PartKind::Text,
            },
            a4mp::StreamEvent::TextDelta {
                index: 0,
                text: "hi".to_owned(),
            },
            a4mp::StreamEvent::PartFinished { index: 0 },
            a4mp::StreamEvent::Finished {
                stop: Some(StopReason::Complete),
            },
        ] {
            text.extend_from_slice(&relay.push(&event));
        }
        text.extend_from_slice(&relay.finish());
        let text = String::from_utf8(text).unwrap();
        assert!(text.contains("hi"));
        assert!(text.contains("[DONE]"));
    }

    #[test]
    fn openai_chat_completions_json_and_stream_are_canonical_round_trips() {
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
        assert!(matches!(
            request.request.messages[0].content[0],
            ContentPart::Text { ref text } if text == "hello"
        ));

        let output = a4mp::protocols::openai_chat::decode_response(
            br#"{"id":"chat-1","choices":[{"message":{"role":"assistant","content":"done"}}]}"#,
        )
        .unwrap();
        let response = gateway.finish(&request, &output).unwrap();
        assert_eq!(response.content_type, "application/json");
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["choices"][0]["message"]["content"], "done");
    }

    #[test]
    fn reasoning_replay_is_bound_to_the_protocol_that_issued_it() {
        let gateway = gateway();
        let first = gateway
            .prepare("/v1/responses", br#"{"model":"k3-256k","input":"first"}"#)
            .unwrap();
        let output = Output {
            parts: vec![
                ContentPart::Reasoning(ReasoningBlock {
                    protocol: Protocol::AnthropicMessages,
                    opaque: json!({"signature":"fixture"}),
                    summary: None,
                }),
                ContentPart::Text {
                    text: "answer".to_owned(),
                },
            ],
            ..Default::default()
        };
        // Recorded history keeps the foreign-protocol block; on splice it is
        // filtered down to what the Chat upstream itself could have issued.
        let response = gateway.finish(&first, &output).unwrap();
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        let id = body["id"].as_str().unwrap().to_owned();
        let second = gateway
            .prepare(
                "/v1/responses",
                serde_json::to_vec(&json!({
                    "model":"k3-256k","input":"second","previous_response_id":id
                }))
                .unwrap()
                .as_slice(),
            )
            .unwrap();
        let assistant = second
            .request
            .messages
            .iter()
            .find(|message| message.role == Role::Assistant)
            .unwrap();
        assert!(
            assistant
                .content
                .iter()
                .all(|part| !matches!(part, ContentPart::Reasoning(_))),
            "an Anthropic-issued reasoning block is foreign to a Chat upstream"
        );
        assert!(
            assistant
                .content
                .iter()
                .any(|part| matches!(part, ContentPart::Text { text } if text == "answer"))
        );
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
        let first_response = gateway
            .finish(
                &first,
                &Output {
                    parts: vec![ContentPart::Text {
                        text: "answer".to_owned(),
                    }],
                    stop: Some(StopReason::Complete),
                    ..Default::default()
                },
            )
            .unwrap();
        let first_body: Value = serde_json::from_slice(&first_response.body).unwrap();
        let previous_id = first_body["id"].as_str().unwrap().to_owned();
        let second = gateway
            .prepare(
                "/v1/responses",
                serde_json::to_string(&json!({
                    "model":"k3-256k","input":"second","previous_response_id":previous_id
                }))
                .unwrap()
                .as_bytes(),
            )
            .unwrap();
        let texts = second
            .request
            .messages
            .iter()
            .flat_map(|message| {
                message.content.iter().filter_map(|part| match part {
                    ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(texts, vec!["first", "answer", "second"]);
        assert!(
            second
                .request
                .messages
                .iter()
                .all(|message| message.role != Role::System)
        );
    }

    /// A second streamed Responses turn must continue from the first turn's
    /// `previous_response_id` on every non-Responses upstream, exactly as the
    /// buffered path does.
    #[test]
    fn streamed_responses_on_anthropic_history_continues_the_next_turn() {
        let gateway = gateway();
        let first = gateway
            .prepare(
                "/v1/responses",
                serde_json::to_vec(&json!({
                    "model": "responses-on-anthropic",
                    "input": "first",
                    "stream": true,
                }))
                .unwrap()
                .as_slice(),
            )
            .unwrap();
        assert_eq!(first.upstream_protocol, UpstreamProtocol::AnthropicMessages);

        let mut relay = crate::model::llm_gateway_stream::ClientStreamRelay::new(&first);
        for event in [
            a4mp::StreamEvent::PartStarted {
                index: 0,
                kind: a4mp::PartKind::Text,
            },
            a4mp::StreamEvent::TextDelta {
                index: 0,
                text: "hi".to_owned(),
            },
            a4mp::StreamEvent::PartFinished { index: 0 },
            a4mp::StreamEvent::Finished {
                stop: Some(a4mp::StopReason::Complete),
            },
        ] {
            relay.push(&event);
        }
        relay.finish();
        let history = relay
            .take_history()
            .expect("the streamed Responses turn recorded history");
        let previous_id = history.response_id.clone();
        gateway.remember_stream_response(&first, history).unwrap();

        let second = gateway
            .prepare(
                "/v1/responses",
                serde_json::to_vec(&json!({
                    "model": "responses-on-anthropic",
                    "input": "second",
                    "stream": true,
                    "previous_response_id": previous_id,
                }))
                .unwrap()
                .as_slice(),
            )
            .expect("a streamed first turn is continuable");
        let texts = second
            .request
            .messages
            .iter()
            .flat_map(|message| {
                message.content.iter().filter_map(|part| match part {
                    ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(texts, vec!["first", "hi", "second"]);
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
        let mut relay = crate::model::llm_gateway_stream::ClientStreamRelay::new(&request);
        let mut text = relay.preamble();
        for event in [
            a4mp::StreamEvent::ToolCallStarted {
                index: 0,
                id: "tool-1".to_owned(),
                name: "browser".to_owned(),
            },
            a4mp::StreamEvent::ToolCallDelta {
                index: 0,
                arguments: "{\"url\":\"https://example.invalid\"}".to_owned(),
            },
            a4mp::StreamEvent::PartFinished { index: 0 },
            a4mp::StreamEvent::Finished {
                stop: Some(a4mp::StopReason::ToolUse),
            },
        ] {
            text.extend_from_slice(&relay.push(&event));
        }
        text.extend_from_slice(&relay.finish());
        let text = String::from_utf8(text).unwrap();
        assert!(text.contains("content_block_start") || text.contains("tool_use"));
        assert!(text.contains("message_stop") || text.contains("input_json_delta"));
    }

    #[test]
    fn responses_custom_tools_fold_into_function_definitions() {
        let gateway = gateway();
        let request = gateway
            .prepare(
                "/v1/responses",
                serde_json::to_vec(&json!({
                    "model":"k3-256k","input":"edit",
                    "tools":[
                        {"type":"custom","name":"apply_patch","description":"edit files","format":{"type":"grammar","syntax":"lark","definition":"x"}}
                    ]
                }))
                .unwrap()
                .as_slice(),
            )
            .unwrap();
        let tool = request
            .request
            .tools
            .iter()
            .find(|tool| tool.name == "apply_patch")
            .expect("custom tool folded into a function definition");
        assert!(tool.description.contains("Codex custom tool metadata"));
    }

    #[test]
    fn configuration_and_unknown_routes_fail_closed() {
        let gateway = gateway();
        assert_eq!(
            gateway
                .prepare("/v1/messages", br#"{"model":"unknown","messages":[]}"#)
                .unwrap_err(),
            GatewayError::RouteNotFound
        );
        assert_eq!(
            gateway
                .prepare(
                    "/v1/chat/completions",
                    br#"{"model":"unknown-provider:model","messages":[]}"#
                )
                .unwrap_err(),
            GatewayError::RouteNotFound
        );
        assert_eq!(
            gateway
                .prepare(
                    "/v1/chat/completions",
                    br#"{"model":"kimi-chat:","messages":[]}"#
                )
                .unwrap_err(),
            GatewayError::RouteNotFound
        );
        assert_eq!(
            gateway
                .prepare(
                    "/v1/responses",
                    br#"{"model":"k3-256k","input":"x","previous_response_id":"resp-old"}"#
                )
                .unwrap_err(),
            GatewayError::UnsupportedConversion
        );
        assert_eq!(
            gateway
                .prepare(
                    "/v1/responses",
                    br#"{"model":"k3-256k","input":"x","reasoning":{"effort":"invented"}}"#
                )
                .unwrap_err(),
            GatewayError::InvalidRequest
        );
    }
}
