//! Bounded outbound transport for the local LLM gateway.

use crate::credential::GatewayCredentialSlot;
use crate::model::llm_gateway::{
    CompiledGateway, CredentialStyle, GatewayError, GatewayProvider, GatewayResponse,
    MAX_GATEWAY_BODY_BYTES, UpstreamProtocol, models_endpoint_for, namespaced_model_id,
};
use crate::model::llm_gateway_stream::GatewayStreamTransformer;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_IN_FLIGHT: usize = 16;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const MODEL_CATALOG_TIMEOUT: Duration = Duration::from_secs(15);
const MODEL_CATALOG_TOTAL_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_COALESCED_WRITE_BYTES: usize = 16 * 1024;
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// Immutable policy selecting one shared outbound agent. Agents are keyed by
/// this policy so warm traffic reuses a connection pool per policy while
/// authorization headers and bodies remain strictly per-call values.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct GatewayAgentPolicy {
    connect_timeout: Duration,
    read_timeout: Duration,
    write_timeout: Duration,
}

impl GatewayAgentPolicy {
    fn loopback_default() -> Self {
        Self {
            connect_timeout: CONNECT_TIMEOUT,
            read_timeout: REQUEST_TIMEOUT,
            write_timeout: REQUEST_TIMEOUT,
        }
    }

    fn model_catalog() -> Self {
        Self {
            connect_timeout: CONNECT_TIMEOUT,
            read_timeout: MODEL_CATALOG_TIMEOUT,
            write_timeout: MODEL_CATALOG_TIMEOUT,
        }
    }
}

/// Serving-lifetime owner of one outbound agent per immutable policy. The
/// gateway runtime process is the serving lifetime, so the shared default pool
/// satisfies the ownership contract while legacy call sites keep their shape.
#[derive(Default)]
struct GatewayAgentPool {
    agents: Mutex<BTreeMap<GatewayAgentPolicy, ureq::Agent>>,
}

impl GatewayAgentPool {
    fn new() -> Self {
        Self::default()
    }

    fn agent_for(&self, policy: GatewayAgentPolicy) -> ureq::Agent {
        let mut agents = match self.agents.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        agents
            .entry(policy)
            .or_insert_with(|| build_agent(policy))
            .clone()
    }
}

fn build_agent(policy: GatewayAgentPolicy) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(policy.connect_timeout)
        .timeout_read(policy.read_timeout)
        .timeout_write(policy.write_timeout)
        .build()
}

fn default_agents() -> &'static GatewayAgentPool {
    static AGENTS: OnceLock<GatewayAgentPool> = OnceLock::new();
    AGENTS.get_or_init(GatewayAgentPool::new)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatewayTransportError {
    Gateway(GatewayError),
    CredentialUnavailable,
    Busy,
    TransportFailed,
    ResponseTooLarge,
}

pub trait GatewayStreamSink {
    fn begin(&mut self, status: u16, content_type: &str) -> std::io::Result<()>;
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()>;
}

#[derive(Debug)]
pub enum GatewayExchange {
    Buffered(GatewayResponse),
    Streamed,
}

/// Fetch the current catalog from every provider whose credential is present in
/// the live lease. The upstream model object is preserved, while `id` is
/// namespaced with the provider lane so a later inference request is
/// unambiguous.
pub fn list_models(
    gateway: &CompiledGateway,
    credentials: &GatewayCredentialSlot,
) -> Result<GatewayResponse, GatewayTransportError> {
    let _permit = Permit::acquire()?;
    let agent = default_agents().agent_for(GatewayAgentPolicy::model_catalog());
    let deadline = Instant::now() + MODEL_CATALOG_TOTAL_TIMEOUT;
    let mut models = Vec::new();
    let mut seen = BTreeSet::new();
    let mut successful_providers = 0usize;
    let mut first_failure = None;
    let mut failed_providers = 0usize;
    for provider in gateway.catalog_providers() {
        if !credentials.contains_kind(provider.credential_provider) {
            continue;
        }
        let candidates = match credentials.resolve_candidates(provider.credential_provider) {
            Ok(candidates) => candidates,
            Err(_) => {
                failed_providers += 1;
                first_failure.get_or_insert(CatalogFailure::Transport(
                    GatewayTransportError::CredentialUnavailable,
                ));
                continue;
            }
        };
        match fetch_provider_models(&agent, &provider, candidates, deadline) {
            Err(error) => {
                failed_providers += 1;
                first_failure.get_or_insert(CatalogFailure::Transport(error));
            }
            Ok(ProviderCatalog::Models(provider_models)) => {
                let Ok(provider_models) = project_provider_models(&provider, provider_models)
                else {
                    failed_providers += 1;
                    first_failure.get_or_insert(CatalogFailure::Transport(
                        GatewayTransportError::TransportFailed,
                    ));
                    continue;
                };
                successful_providers += 1;
                for (requested_id, model) in provider_models {
                    if !seen.insert(requested_id.clone()) {
                        continue;
                    }
                    models.push(model);
                }
            }
            Ok(ProviderCatalog::UpstreamError(response)) => {
                failed_providers += 1;
                first_failure.get_or_insert(CatalogFailure::Upstream(response));
            }
        }
    }
    if successful_providers == 0 {
        match first_failure {
            Some(CatalogFailure::Transport(error)) => return Err(error),
            Some(CatalogFailure::Upstream(response)) => return Ok(response),
            None => {}
        }
    }
    let mut document = json!({"object":"list", "data":models});
    if failed_providers > 0 {
        document["partial"] = Value::Bool(true);
        document["failed_provider_count"] = json!(failed_providers);
    }
    let body = serde_json::to_vec(&document).map_err(|_| GatewayTransportError::TransportFailed)?;
    if body.len() > MAX_GATEWAY_BODY_BYTES {
        return Err(GatewayTransportError::ResponseTooLarge);
    }
    Ok(GatewayResponse {
        status: 200,
        content_type: "application/json",
        body,
    })
}

enum ProviderCatalog {
    Models(Vec<Value>),
    UpstreamError(GatewayResponse),
}

enum CatalogFailure {
    Transport(GatewayTransportError),
    Upstream(GatewayResponse),
}

fn project_provider_models(
    provider: &GatewayProvider,
    provider_models: Vec<Value>,
) -> Result<Vec<(String, Value)>, GatewayTransportError> {
    let mut projected = Vec::with_capacity(provider_models.len());
    let mut seen = BTreeSet::new();
    for mut model in provider_models {
        let object = model
            .as_object_mut()
            .ok_or(GatewayTransportError::TransportFailed)?;
        let upstream_id = object
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(GatewayTransportError::TransportFailed)?;
        let Some(requested_id) = namespaced_model_id(&provider.id, &upstream_id) else {
            continue;
        };
        if !seen.insert(requested_id.clone()) {
            continue;
        }
        object.insert("id".into(), Value::String(requested_id.clone()));
        object.insert("upstream_id".into(), Value::String(upstream_id));
        object.insert(
            "gateway_provider".into(),
            Value::String(provider.id.clone()),
        );
        object
            .entry("object")
            .or_insert_with(|| Value::String("model".into()));
        object
            .entry("owned_by")
            .or_insert_with(|| Value::String(provider.id.clone()));
        projected.push((requested_id, model));
    }
    Ok(projected)
}

fn fetch_provider_models(
    agent: &ureq::Agent,
    provider: &GatewayProvider,
    credentials: Vec<licoup_foundation::core::secret_bytes::SecretBytes>,
    deadline: Instant,
) -> Result<ProviderCatalog, GatewayTransportError> {
    let endpoint = models_endpoint_for(provider).map_err(GatewayTransportError::Gateway)?;
    let candidate_count = credentials.len();
    for (candidate_index, credential) in credentials.into_iter().enumerate() {
        let timeout = remaining_catalog_timeout(deadline)?;
        let credential = credential
            .expose_utf8()
            .map_err(|_| GatewayTransportError::CredentialUnavailable)?;
        let mut request = agent
            .get(&endpoint)
            .timeout(timeout)
            .set("accept", "application/json");
        request = match provider.credential_style {
            CredentialStyle::Bearer => {
                request.set("authorization", &format!("Bearer {credential}"))
            }
            CredentialStyle::XApiKey => request.set("x-api-key", &credential),
        };
        let has_fallback = candidate_index + 1 < candidate_count;
        let response = match request.call() {
            Ok(response) => response,
            Err(ureq::Error::Status(status, response)) => {
                if has_fallback && retryable_status(status) {
                    continue;
                }
                response
            }
            Err(ureq::Error::Transport(_)) if has_fallback => continue,
            Err(ureq::Error::Transport(_)) => {
                return Err(GatewayTransportError::TransportFailed);
            }
        };
        let status = response.status();
        let body = read_bounded_response(response)?;
        if !(200..300).contains(&status) {
            return Ok(ProviderCatalog::UpstreamError(model_catalog_error(
                status, &body,
            )));
        }
        let document: Value =
            serde_json::from_slice(&body).map_err(|_| GatewayTransportError::TransportFailed)?;
        let models = document
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .ok_or(GatewayTransportError::TransportFailed)?;
        return Ok(ProviderCatalog::Models(models));
    }
    Err(GatewayTransportError::CredentialUnavailable)
}

fn remaining_catalog_timeout(deadline: Instant) -> Result<Duration, GatewayTransportError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(GatewayTransportError::TransportFailed);
    }
    Ok(remaining.min(MODEL_CATALOG_TIMEOUT))
}

fn read_bounded_response(response: ureq::Response) -> Result<Vec<u8>, GatewayTransportError> {
    let mut body = Vec::new();
    response
        .into_reader()
        .take((MAX_GATEWAY_BODY_BYTES as u64).saturating_add(1))
        .read_to_end(&mut body)
        .map_err(|_| GatewayTransportError::TransportFailed)?;
    if body.len() > MAX_GATEWAY_BODY_BYTES {
        return Err(GatewayTransportError::ResponseTooLarge);
    }
    Ok(body)
}

fn model_catalog_error(status: u16, body: &[u8]) -> GatewayResponse {
    let parsed = serde_json::from_slice::<Value>(body).ok();
    let message = parsed
        .as_ref()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("message"))
        })
        .and_then(Value::as_str)
        .unwrap_or("upstream model catalog request failed");
    let message = message.chars().take(1024).collect::<String>();
    GatewayResponse {
        status,
        content_type: "application/json",
        body: serde_json::to_vec(
            &json!({"error":{"message":message,"type":"upstream_error","code":status,"param":null}}),
        )
        .unwrap_or_default(),
    }
}

/// Routes, converts, authorizes, and executes one model request. Credential
/// lookup happens only after the closed route and endpoint have been accepted.
pub fn exchange(
    gateway: &CompiledGateway,
    path: &str,
    body: &[u8],
    incoming_user_agent: Option<&str>,
    incoming_anthropic_beta: Option<&str>,
    credentials: &GatewayCredentialSlot,
) -> Result<GatewayResponse, GatewayTransportError> {
    match exchange_inner(
        gateway,
        path,
        body,
        incoming_user_agent,
        incoming_anthropic_beta,
        credentials,
        None,
    )? {
        GatewayExchange::Buffered(response) => Ok(response),
        GatewayExchange::Streamed => Err(GatewayTransportError::TransportFailed),
    }
}

pub fn exchange_to_sink(
    gateway: &CompiledGateway,
    path: &str,
    body: &[u8],
    incoming_user_agent: Option<&str>,
    incoming_anthropic_beta: Option<&str>,
    credentials: &GatewayCredentialSlot,
    sink: &mut dyn GatewayStreamSink,
) -> Result<GatewayExchange, GatewayTransportError> {
    exchange_inner(
        gateway,
        path,
        body,
        incoming_user_agent,
        incoming_anthropic_beta,
        credentials,
        Some(sink),
    )
}

fn exchange_inner(
    gateway: &CompiledGateway,
    path: &str,
    body: &[u8],
    incoming_user_agent: Option<&str>,
    incoming_anthropic_beta: Option<&str>,
    credentials: &GatewayCredentialSlot,
    mut sink: Option<&mut dyn GatewayStreamSink>,
) -> Result<GatewayExchange, GatewayTransportError> {
    let prepared = gateway
        .prepare(path, body)
        .map_err(GatewayTransportError::Gateway)?;
    let credential_candidates = credentials
        .resolve_candidates(prepared.credential_provider)
        .map_err(|_| GatewayTransportError::CredentialUnavailable)?;
    let _permit = Permit::acquire()?;
    let agent = default_agents().agent_for(GatewayAgentPolicy::loopback_default());
    let candidate_count = credential_candidates.len();
    for (candidate_index, credential) in credential_candidates.into_iter().enumerate() {
        let credential = credential
            .expose_utf8()
            .map_err(|_| GatewayTransportError::CredentialUnavailable)?;
        let mut request = agent
            .post(&prepared.endpoint)
            .set(
                "accept",
                if prepared.stream {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            )
            .set("content-type", "application/json");
        if let Some(user_agent) = incoming_user_agent.filter(|value| valid_header_value(value)) {
            request = request.set("user-agent", user_agent);
        }
        if prepared.upstream_protocol == UpstreamProtocol::AnthropicMessages {
            request = request.set("anthropic-version", "2023-06-01");
            if let Some(beta) = incoming_anthropic_beta.filter(|value| valid_header_value(value)) {
                request = request.set("anthropic-beta", beta);
            }
        }
        request = match prepared.credential_style {
            CredentialStyle::Bearer => {
                request.set("authorization", &format!("Bearer {credential}"))
            }
            CredentialStyle::XApiKey => request.set("x-api-key", &credential),
        };
        let has_fallback = candidate_index + 1 < candidate_count;
        let response = match request.send_bytes(&prepared.body) {
            Ok(response) => response,
            Err(ureq::Error::Status(status, response)) => {
                if has_fallback && retryable_status(status) {
                    continue;
                }
                response
            }
            Err(ureq::Error::Transport(_)) if has_fallback => continue,
            Err(ureq::Error::Transport(_)) => {
                return Err(GatewayTransportError::TransportFailed);
            }
        };
        let status = response.status();
        let content_type = response.header("content-type").map(str::to_owned);
        let is_sse = content_type
            .as_deref()
            .is_some_and(|value| value.to_ascii_lowercase().contains("text/event-stream"));
        if prepared.stream && (200..300).contains(&status) && is_sse {
            if let Some(sink) = sink.as_deref_mut() {
                let mut transformer = GatewayStreamTransformer::new(&prepared)
                    .map_err(GatewayTransportError::Gateway)?;
                let mut coalescer = SseFrameCoalescer::new();
                sink.begin(status, "text/event-stream")
                    .map_err(|_| GatewayTransportError::TransportFailed)?;
                let mut reader = response.into_reader();
                let mut chunk = [0u8; 8192];
                loop {
                    let read = reader
                        .read(&mut chunk)
                        .map_err(|_| GatewayTransportError::TransportFailed)?;
                    if read == 0 {
                        break;
                    }
                    let converted = transformer
                        .push(&chunk[..read])
                        .map_err(GatewayTransportError::Gateway)?;
                    if !converted.is_empty() {
                        coalescer
                            .push(&converted, sink)
                            .map_err(|_| GatewayTransportError::TransportFailed)?;
                    }
                }
                let final_bytes = transformer
                    .finish()
                    .map_err(GatewayTransportError::Gateway)?;
                if !final_bytes.is_empty() {
                    coalescer
                        .push(&final_bytes, sink)
                        .map_err(|_| GatewayTransportError::TransportFailed)?;
                }
                coalescer
                    .finish(sink)
                    .map_err(|_| GatewayTransportError::TransportFailed)?;
                if let Some(history_response) = transformer.take_chat_history_response() {
                    gateway
                        .remember_stream_response(&prepared, history_response)
                        .map_err(GatewayTransportError::Gateway)?;
                }
                return Ok(GatewayExchange::Streamed);
            }
        }
        let mut response_body = Vec::new();
        response
            .into_reader()
            .take((MAX_GATEWAY_BODY_BYTES as u64).saturating_add(1))
            .read_to_end(&mut response_body)
            .map_err(|_| GatewayTransportError::TransportFailed)?;
        if response_body.len() > MAX_GATEWAY_BODY_BYTES {
            return Err(GatewayTransportError::ResponseTooLarge);
        }
        return gateway
            .finish(&prepared, status, content_type.as_deref(), &response_body)
            .map(GatewayExchange::Buffered)
            .map_err(GatewayTransportError::Gateway);
    }
    Err(GatewayTransportError::CredentialUnavailable)
}

fn retryable_status(status: u16) -> bool {
    matches!(status, 401 | 403 | 429) || (500..=599).contains(&status)
}

fn valid_header_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && value
            .bytes()
            .all(|byte| byte == b'\t' || (0x20..=0x7e).contains(&byte))
}

/// Coalesces converted SSE output into complete frames. Fragment writes never
/// reach the sink: a batch holds whole frames only and stays below the write
/// cap unless a single frame is itself larger than the cap.
struct SseFrameCoalescer {
    pending: Vec<u8>,
}

impl SseFrameCoalescer {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    fn push(&mut self, bytes: &[u8], sink: &mut dyn GatewayStreamSink) -> std::io::Result<()> {
        self.pending.extend_from_slice(bytes);
        self.flush_complete(sink)
    }

    fn flush_complete(&mut self, sink: &mut dyn GatewayStreamSink) -> std::io::Result<()> {
        while let Some(first_end) = complete_frame_end(&self.pending) {
            let mut end = first_end;
            while end < self.pending.len() {
                let Some(relative) = complete_frame_end(&self.pending[end..]) else {
                    break;
                };
                let next = end + relative;
                if next > MAX_COALESCED_WRITE_BYTES {
                    break;
                }
                end = next;
            }
            sink.write(&self.pending[..end])?;
            self.pending.drain(..end);
        }
        Ok(())
    }

    /// Terminal flush. A trailing partial frame is an upstream truncation and
    /// fails instead of writing a fragment; trailing whitespace is retained.
    fn finish(&mut self, sink: &mut dyn GatewayStreamSink) -> std::io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let complete = complete_frame_end(&self.pending) == Some(self.pending.len())
            || self.pending.iter().all(u8::is_ascii_whitespace);
        if !complete {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "incomplete_sse_frame",
            ));
        }
        sink.write(&self.pending)?;
        self.pending.clear();
        Ok(())
    }
}

fn complete_frame_end(bytes: &[u8]) -> Option<usize> {
    let lf = bytes
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|position| position + 2);
    let crlf = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4);
    match (lf, crlf) {
        (Some(lf_end), Some(crlf_end)) => Some(lf_end.min(crlf_end)),
        (Some(lf_end), None) => Some(lf_end),
        (None, Some(crlf_end)) => Some(crlf_end),
        (None, None) => None,
    }
}

struct Permit;

impl Permit {
    fn acquire() -> Result<Self, GatewayTransportError> {
        IN_FLIGHT
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < MAX_IN_FLIGHT).then_some(active + 1)
            })
            .map(|_| Self)
            .map_err(|_| GatewayTransportError::Busy)
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::{
        GatewayCredential, GatewayCredentialEpochSource, GatewayCredentialLease,
        GatewayCredentialLeaseDays, LlmApiKeyProvider,
    };
    use crate::model::llm_gateway::{
        ClientProtocol, CredentialStyle, GatewayConfig, GatewayProvider, ModelRoute,
    };
    use anyhow::Result;
    use licoup_foundation::core::secret_bytes::SecretBytes;
    use std::collections::BTreeMap;
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::sync::Arc;

    struct StaticEpochSource(String);

    impl GatewayCredentialEpochSource for StaticEpochSource {
        fn active_epoch(&self) -> Result<Option<String>> {
            Ok(Some(self.0.clone()))
        }
    }

    struct ChannelSink(std::sync::mpsc::Sender<Vec<u8>>);

    impl GatewayStreamSink for ChannelSink {
        fn begin(&mut self, _status: u16, _content_type: &str) -> std::io::Result<()> {
            Ok(())
        }

        fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            self.0.send(bytes.to_vec()).unwrap();
            Ok(())
        }
    }

    fn fixture_gateway(address: std::net::SocketAddr) -> CompiledGateway {
        CompiledGateway::compile(GatewayConfig {
            schema_version: 1,
            providers: vec![GatewayProvider {
                id: "fixture".to_owned(),
                base_url: format!("http://{address}/v1"),
                protocol: UpstreamProtocol::OpenAiChatCompletions,
                credential_provider: LlmApiKeyProvider::Kimi,
                credential_style: CredentialStyle::Bearer,
            }],
            routes: vec![ModelRoute {
                client_protocol: ClientProtocol::OpenAiChatCompletions,
                requested_model: "requested".to_owned(),
                provider_id: "fixture".to_owned(),
                upstream_model: "upstream".to_owned(),
            }],
        })
        .unwrap()
    }

    fn responses_gateway(address: std::net::SocketAddr) -> CompiledGateway {
        CompiledGateway::compile(GatewayConfig {
            schema_version: 1,
            providers: vec![GatewayProvider {
                id: "fixture".to_owned(),
                base_url: format!("http://{address}/v1"),
                protocol: UpstreamProtocol::OpenAiChatCompletions,
                credential_provider: LlmApiKeyProvider::Kimi,
                credential_style: CredentialStyle::Bearer,
            }],
            routes: vec![ModelRoute {
                client_protocol: ClientProtocol::OpenAiResponses,
                requested_model: "requested".to_owned(),
                provider_id: "fixture".to_owned(),
                upstream_model: "upstream".to_owned(),
            }],
        })
        .unwrap()
    }

    fn fixture_slot(secrets: &[&str]) -> GatewayCredentialSlot {
        fixture_slot_for(LlmApiKeyProvider::Kimi, secrets)
    }

    fn fixture_slot_for(provider: LlmApiKeyProvider, secrets: &[&str]) -> GatewayCredentialSlot {
        fixture_slot_for_providers(
            secrets
                .iter()
                .map(|secret| (provider, *secret))
                .collect::<Vec<_>>(),
        )
    }

    fn fixture_slot_for_providers(
        entries: Vec<(LlmApiKeyProvider, &str)>,
    ) -> GatewayCredentialSlot {
        let epoch = uuid::Uuid::new_v4().to_string();
        let mut credentials = BTreeMap::<LlmApiKeyProvider, Vec<GatewayCredential>>::new();
        for (provider, secret) in entries {
            credentials.entry(provider).or_default().push(
                GatewayCredential::new(
                    uuid::Uuid::new_v4().to_string(),
                    SecretBytes::try_from_string(secret.to_string()).unwrap(),
                    None,
                )
                .unwrap(),
            );
        }
        GatewayCredentialSlot::new(
            GatewayCredentialLease::new(
                credentials,
                GatewayCredentialLeaseDays::Seven.duration(),
                epoch.clone(),
                Arc::new(StaticEpochSource(epoch)),
            )
            .unwrap(),
        )
    }

    fn read_one_request(stream: &mut impl Read) -> Vec<u8> {
        let mut received = Vec::with_capacity(512);
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            let read = stream.read(&mut chunk).unwrap();
            assert!(
                read > 0,
                "fake upstream closed before the request completed"
            );
            received.extend_from_slice(&chunk[..read]);
            if let Some(position) = received.windows(4).position(|part| part == b"\r\n\r\n") {
                break position;
            }
        };
        let head = String::from_utf8_lossy(&received[..header_end]);
        let content_length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        let body_start = header_end + 4;
        while received.len().saturating_sub(body_start) < content_length {
            let mut chunk = [0u8; 4096];
            let read = stream.read(&mut chunk).unwrap();
            assert!(
                read > 0,
                "fake upstream closed before the request body completed"
            );
            received.extend_from_slice(&chunk[..read]);
        }
        received.truncate(body_start + content_length);
        received
    }

    #[test]
    fn user_agent_forwarding_is_bounded_and_header_safe() {
        assert!(valid_header_value("codex-cli/1.0"));
        assert!(!valid_header_value("codex\r\nx-api-key: secret"));
        assert!(!valid_header_value(&"x".repeat(1025)));
    }

    #[test]
    fn model_catalog_request_timeout_respects_the_aggregate_deadline() {
        assert_eq!(
            remaining_catalog_timeout(Instant::now() - Duration::from_millis(1)),
            Err(GatewayTransportError::TransportFailed)
        );
        assert!(
            remaining_catalog_timeout(Instant::now() + Duration::from_secs(60)).unwrap()
                <= MODEL_CATALOG_TIMEOUT
        );
    }

    #[test]
    fn model_catalog_forwards_the_live_provider_response_with_namespaced_ids() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_one_request(&mut stream);
            let request = String::from_utf8_lossy(&request);
            assert!(request.starts_with("GET /v1/models HTTP/1.1"));
            assert!(request.contains("authorization: Bearer fixture-model-key"));
            let body = r#"{"object":"list","data":[{"id":"upstream-new","object":"model","owned_by":"vendor","name":"Upstream New","context_length":262144}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let gateway = fixture_gateway(address);
        let slot = fixture_slot(&["fixture-model-key"]);

        let response = list_models(&gateway, &slot).unwrap();

        assert_eq!(response.status, 200);
        let document: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(document["object"], "list");
        assert_eq!(document["data"][0]["id"], "fixture:upstream-new");
        assert_eq!(document["data"][0]["upstream_id"], "upstream-new");
        assert_eq!(document["data"][0]["gateway_provider"], "fixture");
        assert_eq!(document["data"][0]["owned_by"], "vendor");
        assert_eq!(document["data"][0]["context_length"], 262144);
        server.join().unwrap();
    }

    #[test]
    fn model_catalog_without_an_authorized_provider_is_empty() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let gateway = fixture_gateway(listener.local_addr().unwrap());
        let slot = GatewayCredentialSlot::disconnected();

        let response = list_models(&gateway, &slot).unwrap();

        assert_eq!(response.status, 200);
        let document: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(document["data"], json!([]));
    }

    #[test]
    fn model_catalog_queries_only_the_provider_selected_by_the_authorized_key() {
        for (selected, expected_provider_id) in [
            (LlmApiKeyProvider::Kimi, "kimi-lane"),
            (LlmApiKeyProvider::DeepSeek, "deepseek-lane"),
        ] {
            let selected_listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let selected_address = selected_listener.local_addr().unwrap();
            let unused_listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let unused_address = unused_listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = selected_listener.accept().unwrap();
                let _request = read_one_request(&mut stream);
                let body = r#"{"object":"list","data":[{"id":"selected-model"}]}"#;
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            });
            let provider =
                |id: &str, address: std::net::SocketAddr, credential_provider| GatewayProvider {
                    id: id.into(),
                    base_url: format!("http://{address}/v1"),
                    protocol: UpstreamProtocol::OpenAiChatCompletions,
                    credential_provider,
                    credential_style: CredentialStyle::Bearer,
                };
            let gateway = CompiledGateway::compile(GatewayConfig {
                schema_version: 1,
                providers: vec![
                    provider(
                        "kimi-lane",
                        if selected == LlmApiKeyProvider::Kimi {
                            selected_address
                        } else {
                            unused_address
                        },
                        LlmApiKeyProvider::Kimi,
                    ),
                    provider(
                        "deepseek-lane",
                        if selected == LlmApiKeyProvider::DeepSeek {
                            selected_address
                        } else {
                            unused_address
                        },
                        LlmApiKeyProvider::DeepSeek,
                    ),
                ],
                routes: Vec::new(),
            })
            .unwrap();
            let slot = fixture_slot_for(selected, &["provider-specific-key"]);

            let response = list_models(&gateway, &slot).unwrap();

            let document: Value = serde_json::from_slice(&response.body).unwrap();
            assert_eq!(document["data"].as_array().unwrap().len(), 1);
            assert_eq!(
                document["data"][0]["id"],
                format!("{expected_provider_id}:selected-model")
            );
            server.join().unwrap();
            drop(unused_listener);
        }
    }

    #[test]
    fn invalid_provider_key_returns_the_real_upstream_failure_not_a_catalog() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _request = read_one_request(&mut stream);
            let body = r#"{"error":{"message":"invalid provider key"}}"#;
            write!(
                stream,
                "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let gateway = fixture_gateway(address);
        let slot = fixture_slot(&["fixture-invalid-key"]);

        let response = list_models(&gateway, &slot).unwrap();

        assert_eq!(response.status, 401);
        let document: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(document["error"]["message"], "invalid provider key");
        assert!(document.get("data").is_none());
        server.join().unwrap();
    }

    #[test]
    fn one_provider_failure_does_not_hide_another_live_catalog() {
        let failing = TcpListener::bind("127.0.0.1:0").unwrap();
        let failing_address = failing.local_addr().unwrap();
        let failing_server = std::thread::spawn(move || {
            let (mut stream, _) = failing.accept().unwrap();
            let _request = read_one_request(&mut stream);
            let body = r#"{"error":{"message":"rejected"}}"#;
            write!(
                stream,
                "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let healthy = TcpListener::bind("127.0.0.1:0").unwrap();
        let healthy_address = healthy.local_addr().unwrap();
        let healthy_server = std::thread::spawn(move || {
            let (mut stream, _) = healthy.accept().unwrap();
            let _request = read_one_request(&mut stream);
            let body = r#"{"data":[{"id":"healthy-model"}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let provider = |id: &str, address, credential_provider| GatewayProvider {
            id: id.into(),
            base_url: format!("http://{address}/v1"),
            protocol: UpstreamProtocol::OpenAiChatCompletions,
            credential_provider,
            credential_style: CredentialStyle::Bearer,
        };
        let gateway = CompiledGateway::compile(GatewayConfig {
            schema_version: 1,
            providers: vec![
                provider("a-failing", failing_address, LlmApiKeyProvider::DeepSeek),
                provider("z-healthy", healthy_address, LlmApiKeyProvider::Kimi),
            ],
            routes: Vec::new(),
        })
        .unwrap();
        let slot = fixture_slot_for_providers(vec![
            (LlmApiKeyProvider::DeepSeek, "invalid-deepseek-key"),
            (LlmApiKeyProvider::Kimi, "valid-kimi-key"),
        ]);

        let response = list_models(&gateway, &slot).unwrap();

        assert_eq!(response.status, 200);
        let document: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(document["data"].as_array().unwrap().len(), 1);
        assert_eq!(document["data"][0]["id"], "z-healthy:healthy-model");
        assert_eq!(document["partial"], true);
        assert_eq!(document["failed_provider_count"], 1);
        failing_server.join().unwrap();
        healthy_server.join().unwrap();
    }

    #[test]
    fn transport_fails_over_to_the_next_authorized_key() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut authorization_headers = Vec::new();
            for status in [401, 200] {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_one_request(&mut stream);
                let request_text = String::from_utf8_lossy(&request);
                authorization_headers.push(
                    request_text
                        .lines()
                        .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
                        .map(str::to_owned)
                        .unwrap(),
                );
                let body = if status == 200 {
                    r#"{"id":"chat-1","object":"chat.completion","created":1,"model":"upstream","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}"#
                } else {
                    r#"{"error":{"message":"rejected"}}"#
                };
                write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
            assert_ne!(authorization_headers[0], authorization_headers[1]);
        });

        let gateway = CompiledGateway::compile(GatewayConfig {
            schema_version: 1,
            providers: vec![GatewayProvider {
                id: "fixture".to_owned(),
                base_url: format!("http://{address}/v1"),
                protocol: UpstreamProtocol::OpenAiChatCompletions,
                credential_provider: LlmApiKeyProvider::Kimi,
                credential_style: CredentialStyle::Bearer,
            }],
            routes: vec![ModelRoute {
                client_protocol: ClientProtocol::OpenAiChatCompletions,
                requested_model: "requested".to_owned(),
                provider_id: "fixture".to_owned(),
                upstream_model: "upstream".to_owned(),
            }],
        })
        .unwrap();
        let epoch = uuid::Uuid::new_v4().to_string();
        let credentials = BTreeMap::from([(
            LlmApiKeyProvider::Kimi,
            ["fixture-first-key", "fixture-second-key"]
                .into_iter()
                .map(|secret| {
                    GatewayCredential::new(
                        uuid::Uuid::new_v4().to_string(),
                        SecretBytes::try_from_string(secret.to_owned()).unwrap(),
                        None,
                    )
                    .unwrap()
                })
                .collect(),
        )]);
        let slot = GatewayCredentialSlot::new(
            GatewayCredentialLease::new(
                credentials,
                GatewayCredentialLeaseDays::Seven.duration(),
                epoch.clone(),
                Arc::new(StaticEpochSource(epoch)),
            )
            .unwrap(),
        );

        let response = exchange(
            &gateway,
            "/v1/chat/completions",
            br#"{"model":"requested","messages":[{"role":"user","content":"hello"}]}"#,
            None,
            None,
            &slot,
        )
        .unwrap();

        assert_eq!(response.status, 200);
        server.join().unwrap();
    }

    #[test]
    fn streaming_transport_delivers_first_event_before_upstream_finishes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (release_sender, release_receiver) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _request = read_one_request(&mut stream);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {{\"id\":\"one\",\"choices\":[{{\"delta\":{{\"content\":\"first\"}}}}]}}\n\n"
            )
            .unwrap();
            stream.flush().unwrap();
            release_receiver
                .recv_timeout(Duration::from_secs(2))
                .unwrap();
            stream.write_all(b"data: [DONE]\n\n").unwrap();
        });

        let gateway = CompiledGateway::compile(GatewayConfig {
            schema_version: 1,
            providers: vec![GatewayProvider {
                id: "fixture".to_owned(),
                base_url: format!("http://{address}/v1"),
                protocol: UpstreamProtocol::OpenAiChatCompletions,
                credential_provider: LlmApiKeyProvider::Kimi,
                credential_style: CredentialStyle::Bearer,
            }],
            routes: vec![ModelRoute {
                client_protocol: ClientProtocol::OpenAiChatCompletions,
                requested_model: "requested".to_owned(),
                provider_id: "fixture".to_owned(),
                upstream_model: "upstream".to_owned(),
            }],
        })
        .unwrap();
        let epoch = uuid::Uuid::new_v4().to_string();
        let credentials = BTreeMap::from([(
            LlmApiKeyProvider::Kimi,
            vec![
                GatewayCredential::new(
                    uuid::Uuid::new_v4().to_string(),
                    SecretBytes::try_from_string("fixture-stream-key".to_owned()).unwrap(),
                    None,
                )
                .unwrap(),
            ],
        )]);
        let slot = GatewayCredentialSlot::new(
            GatewayCredentialLease::new(
                credentials,
                GatewayCredentialLeaseDays::Seven.duration(),
                epoch.clone(),
                Arc::new(StaticEpochSource(epoch)),
            )
            .unwrap(),
        );
        let (event_sender, event_receiver) = std::sync::mpsc::channel();
        let exchange = std::thread::spawn(move || {
            let mut sink = ChannelSink(event_sender);
            exchange_to_sink(
                &gateway,
                "/v1/chat/completions",
                br#"{"model":"requested","stream":true,"messages":[]}"#,
                None,
                None,
                &slot,
                &mut sink,
            )
        });

        let first = event_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("first SSE event before upstream completion");
        assert!(String::from_utf8_lossy(&first).contains("first"));
        release_sender.send(()).unwrap();
        match exchange.join().unwrap() {
            Ok(GatewayExchange::Streamed) => {}
            Ok(GatewayExchange::Buffered(_)) => panic!("stream was buffered"),
            Err(error) => panic!("stream failed: {error:?}"),
        }
        server.join().unwrap();
    }

    #[test]
    fn warm_requests_reuse_one_pooled_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            for _ in 0..2 {
                let _request = read_one_request(&mut stream);
                let body = r#"{"id":"chat-1","object":"chat.completion","created":1,"model":"upstream","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}"#;
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                stream.flush().unwrap();
            }
            listener.set_nonblocking(true).unwrap();
            match listener.accept() {
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                other => panic!("warm traffic opened an extra connection: {other:?}"),
            }
        });

        let gateway = fixture_gateway(address);
        let slot = fixture_slot(&["fixture-warm-key"]);
        for _ in 0..2 {
            let response = exchange(
                &gateway,
                "/v1/chat/completions",
                br#"{"model":"requested","messages":[{"role":"user","content":"hello"}]}"#,
                None,
                None,
                &slot,
            )
            .unwrap();
            assert_eq!(response.status, 200);
        }
        server.join().unwrap();
    }

    #[test]
    fn streaming_coalesces_only_complete_frames_in_bounded_batches() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _request = read_one_request(&mut stream);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            stream.flush().unwrap();
            for index in 0..40 {
                let frame = format!(
                    "data: {{\"id\":\"one\",\"choices\":[{{\"delta\":{{\"content\":\"chunk-{index}\"}}}}]}}\n\n"
                );
                let split = frame.len() / 2;
                stream.write_all(&frame.as_bytes()[..split]).unwrap();
                stream.flush().unwrap();
                stream.write_all(&frame.as_bytes()[split..]).unwrap();
                stream.flush().unwrap();
            }
            stream.write_all(b"data: [DONE]\n\n").unwrap();
            stream.flush().unwrap();
        });

        let gateway = fixture_gateway(address);
        let slot = fixture_slot(&["fixture-stream-key"]);
        let (event_sender, event_receiver) = std::sync::mpsc::channel();
        let exchange = std::thread::spawn(move || {
            let mut sink = ChannelSink(event_sender);
            exchange_to_sink(
                &gateway,
                "/v1/chat/completions",
                br#"{"model":"requested","stream":true,"messages":[]}"#,
                None,
                None,
                &slot,
                &mut sink,
            )
        });

        let mut received = Vec::new();
        let mut writes = Vec::new();
        while let Ok(chunk) = event_receiver.recv_timeout(Duration::from_secs(2)) {
            writes.push(chunk.len());
            received.extend_from_slice(&chunk);
            if chunk.ends_with(b"data: [DONE]\n\n") {
                break;
            }
        }
        match exchange.join().unwrap() {
            Ok(GatewayExchange::Streamed) => {}
            other => panic!("stream did not complete: {other:?}"),
        }
        server.join().unwrap();

        let mut expected = Vec::new();
        for index in 0..40 {
            expected.extend_from_slice(
                format!(
                    "data: {{\"id\":\"one\",\"choices\":[{{\"delta\":{{\"content\":\"chunk-{index}\"}}}}]}}\n\n"
                )
                .as_bytes(),
            );
        }
        expected.extend_from_slice(b"data: [DONE]\n\n");
        assert_eq!(
            received, expected,
            "response bytes must reconstruct exactly"
        );
        assert!(
            writes
                .iter()
                .all(|length| *length <= MAX_COALESCED_WRITE_BYTES),
            "coalesced writes must stay within the frame batch cap"
        );
        let mut offset = 0;
        for length in &writes {
            assert!(
                received[offset..offset + length].ends_with(b"\n\n"),
                "every write must end on a complete SSE frame"
            );
            offset += length;
        }
    }

    #[test]
    fn truncated_upstream_frame_fails_without_fragment_flush() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _request = read_one_request(&mut stream);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            stream.write_all(b"data: {\"id\":\"one\"").unwrap();
            stream.flush().unwrap();
        });

        let gateway = fixture_gateway(address);
        let slot = fixture_slot(&["fixture-stream-key"]);
        let (event_sender, event_receiver) = std::sync::mpsc::channel();
        let exchange = std::thread::spawn(move || {
            let mut sink = ChannelSink(event_sender);
            exchange_to_sink(
                &gateway,
                "/v1/chat/completions",
                br#"{"model":"requested","stream":true,"messages":[]}"#,
                None,
                None,
                &slot,
                &mut sink,
            )
        });

        match exchange.join().unwrap() {
            Err(GatewayTransportError::TransportFailed) => {}
            other => panic!("truncated stream must fail closed: {other:?}"),
        }
        assert!(
            event_receiver.try_recv().is_err(),
            "a partial upstream frame must never reach the sink"
        );
        server.join().unwrap();
    }

    #[test]
    fn streamed_responses_history_stays_exact_for_follow_up_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _request = read_one_request(&mut stream);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            stream
                .write_all(
                    b"data: {\"id\":\"chat-1\",\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"streamed answer\",\"reasoning_content\":\"fixture reasoning\",\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"lookup\",\"arguments\":\"{\\\"q\\\":\\\"fixture\\\"}\"}}]}}]}\n\ndata: [DONE]\n\n",
                )
                .unwrap();
            stream.flush().unwrap();
        });

        let gateway = Arc::new(responses_gateway(address));
        let slot = fixture_slot(&["fixture-stream-key"]);
        let (event_sender, event_receiver) = std::sync::mpsc::channel();
        let gateway_for_exchange = Arc::clone(&gateway);
        let exchange = std::thread::spawn(move || {
            let mut sink = ChannelSink(event_sender);
            exchange_to_sink(
                &gateway_for_exchange,
                "/v1/responses",
                br#"{"model":"requested","stream":true,"input":"first"}"#,
                None,
                None,
                &slot,
                &mut sink,
            )
        });

        // The responses conversion ends with a typed completion event rather
        // than the upstream [DONE] frame, so drain until the exchange thread
        // drops the sender.
        while event_receiver.recv_timeout(Duration::from_secs(2)).is_ok() {}
        match exchange.join().unwrap() {
            Ok(GatewayExchange::Streamed) => {}
            other => panic!("stream did not complete: {other:?}"),
        }
        server.join().unwrap();

        let follow_up = gateway
            .prepare(
                "/v1/responses",
                br#"{"model":"requested","input":"second","previous_response_id":"resp_chat-1"}"#,
            )
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&follow_up.body).unwrap();
        let content = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["role"] == "assistant")
            .and_then(|message| message["content"].as_str())
            .unwrap();
        assert_eq!(content, "streamed answer");
        let assistant = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["role"] == "assistant")
            .unwrap();
        assert_eq!(assistant["reasoning_content"], "fixture reasoning");
        assert_eq!(assistant["tool_calls"][0]["id"], "call-1");
        assert_eq!(assistant["tool_calls"][0]["function"]["name"], "lookup");
        assert_eq!(
            assistant["tool_calls"][0]["function"]["arguments"],
            r#"{"q":"fixture"}"#
        );
    }

    /// The credential boundary belongs to this runtime, so its test lives with
    /// the code it refuses through: a model that resolves to a provider the
    /// process holds no lease for stops here rather than reaching the network.
    /// The case was previously asserted from the kernel, which no longer links
    /// this crate at all.
    #[test]
    fn unleased_provider_model_reaches_the_credential_boundary() {
        let gateway = CompiledGateway::compile(GatewayConfig {
            schema_version: 1,
            providers: vec![GatewayProvider {
                id: "deepseek".to_owned(),
                base_url: "https://api.deepseek.com/v1".to_owned(),
                protocol: UpstreamProtocol::OpenAiChatCompletions,
                credential_provider: LlmApiKeyProvider::DeepSeek,
                credential_style: CredentialStyle::Bearer,
            }],
            routes: vec![ModelRoute {
                client_protocol: ClientProtocol::OpenAiChatCompletions,
                requested_model: "deepseek:deepseek-v4-flash".to_owned(),
                provider_id: "deepseek".to_owned(),
                upstream_model: "deepseek-v4-flash".to_owned(),
            }],
        })
        .unwrap();
        let credentials = crate::credential::GatewayCredentialSlot::disconnected();
        let request = serde_json::json!({
            "model": "deepseek:deepseek-v4-flash",
            "messages": [{"role": "user", "content": "synthetic"}]
        });
        let error = exchange(
            &gateway,
            "/v1/chat/completions",
            &serde_json::to_vec(&request).unwrap(),
            None,
            None,
            &credentials,
        )
        .unwrap_err();
        assert_eq!(error, GatewayTransportError::CredentialUnavailable);
    }
}
