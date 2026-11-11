//! Upstream dispatch for the local LLM gateway.
//!
//! The forwarding core lives in fairliny/a4mp: this module provides the two
//! host ports the core consumes — the outbound [`a4mp::Transport`] backed by
//! ureq and the lease-backed [`a4mp::CredentialResolver`] — plus the pump that
//! writes the client-dialect stream to the sink. Credential lookup happens
//! only after the closed route and endpoint have been accepted; the shell
//! never stores keys and fairliny never sees custody.

use crate::credential::{GatewayCredentialSlot, LlmApiKeyProvider};
use crate::model::llm_gateway::{
    CompiledGateway, GatewayError, GatewayProvider, GatewayResponse, MAX_GATEWAY_BODY_BYTES,
    PreparedGatewayRequest, namespaced_model_id,
};
use crate::model::llm_gateway_stream::ClientStreamRelay;
use a4mp::{
    Adapter, AuthKind, Catalog, Credential, CredentialResolver, CredentialScope, EffectHost,
    HttpRequest, HttpResponse, ModelInfo, RequestControl, Secret, ServiceTarget, StreamingResponse,
    Transport, TransportError,
};
use fairliny_gateway::router::Router;
use futures::channel::mpsc;
use futures::executor::block_on;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_IN_FLIGHT: usize = 16;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const MODEL_CATALOG_TIMEOUT: Duration = Duration::from_secs(15);
const MODEL_CATALOG_TOTAL_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_COALESCED_WRITE_BYTES: usize = 16 * 1024;
const RESPONSE_TOO_LARGE: &str = "licoup_gateway_response_too_large";
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
    fn forward_default() -> Self {
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

// ---------------------------------------------------------------------------
// Host ports fairliny/a4mp consume
// ---------------------------------------------------------------------------

/// The host's outbound port: ureq executes the request, bodies stay bounded,
/// and GETs ride the model-catalog policy so catalog fetches share their pool.
struct UpstreamTransport {
    agents: &'static GatewayAgentPool,
}

fn upstream_transport() -> Arc<UpstreamTransport> {
    static TRANSPORT: OnceLock<Arc<UpstreamTransport>> = OnceLock::new();
    Arc::clone(TRANSPORT.get_or_init(|| {
        Arc::new(UpstreamTransport {
            agents: default_agents(),
        })
    }))
}

#[async_trait::async_trait]
impl Transport for UpstreamTransport {
    async fn send(
        &self,
        request: HttpRequest,
        _ctrl: &RequestControl,
    ) -> Result<HttpResponse, TransportError> {
        send_via_ureq(self.agents, &request)
    }

    async fn send_stream(
        &self,
        mut request: HttpRequest,
        _ctrl: &RequestControl,
    ) -> Result<StreamingResponse, TransportError> {
        // A streamed call declares SSE acceptance — providers honor
        // `stream:true` in the body, and the accept header keeps proxies and
        // stricter upstreams on the streaming path the caller asked for.
        request
            .headers
            .entry(http::header::ACCEPT)
            .or_insert(http::HeaderValue::from_static("text/event-stream"));
        let response = dispatch(self.agents, &request)?;
        let status = http::StatusCode::from_u16(response.status())
            .unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR);
        let headers = response_headers(&response);
        let (mut sender, receiver) = mpsc::channel::<Result<bytes::Bytes, a4mp::Error>>(8);
        std::thread::spawn(move || {
            let mut reader = response.into_reader();
            let mut chunk = [0u8; 8192];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => {
                        if block_on(sender.send(Ok(bytes::Bytes::copy_from_slice(&chunk[..read]))))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = block_on(sender.send(Err(a4mp::Error::Transport(
                            TransportError::Disconnected(error.to_string()),
                        ))));
                        break;
                    }
                }
            }
        });
        Ok(StreamingResponse {
            status,
            headers,
            body: Box::pin(receiver),
        })
    }
}

fn dispatch(
    agents: &GatewayAgentPool,
    request: &HttpRequest,
) -> Result<ureq::Response, TransportError> {
    let agent = agents.agent_for(if request.method == http::Method::GET {
        GatewayAgentPolicy::model_catalog()
    } else {
        GatewayAgentPolicy::forward_default()
    });
    let mut call = agent.request(request.method.as_str(), request.url.as_str());
    for (name, value) in request.headers.iter() {
        let value = value
            .to_str()
            .map_err(|_| TransportError::Io("request header value is not ASCII".into()))?;
        call = call.set(name.as_str(), value);
    }
    match if request.method == http::Method::GET {
        call.call()
    } else {
        call.send_bytes(&request.body)
    } {
        Ok(response) => Ok(response),
        Err(ureq::Error::Status(_, response)) => Ok(response),
        Err(ureq::Error::Transport(error)) => Err(map_ureq_error(&error)),
    }
}

fn send_via_ureq(
    agents: &GatewayAgentPool,
    request: &HttpRequest,
) -> Result<HttpResponse, TransportError> {
    let response = dispatch(agents, request)?;
    let status = http::StatusCode::from_u16(response.status())
        .unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR);
    let headers = response_headers(&response);
    let mut body = Vec::new();
    response
        .into_reader()
        .take((MAX_GATEWAY_BODY_BYTES as u64).saturating_add(1))
        .read_to_end(&mut body)
        .map_err(|error| TransportError::Io(error.to_string()))?;
    if body.len() > MAX_GATEWAY_BODY_BYTES {
        return Err(TransportError::Io(RESPONSE_TOO_LARGE.to_owned()));
    }
    Ok(HttpResponse {
        status,
        headers,
        body: bytes::Bytes::from(body),
    })
}

fn response_headers(response: &ureq::Response) -> http::HeaderMap {
    let mut headers = http::HeaderMap::new();
    for name in response.headers_names() {
        for value in response.all(&name) {
            if let (Ok(name), Ok(value)) = (
                http::HeaderName::from_bytes(name.as_bytes()),
                http::HeaderValue::from_str(value),
            ) {
                headers.append(name, value);
            }
        }
    }
    headers
}

fn map_ureq_error(error: &ureq::Transport) -> TransportError {
    match error.kind() {
        ureq::ErrorKind::ConnectionFailed
        | ureq::ErrorKind::Dns
        | ureq::ErrorKind::ProxyConnect
        | ureq::ErrorKind::ProxyUnauthorized
        | ureq::ErrorKind::TooManyRedirects
        | ureq::ErrorKind::InvalidUrl => TransportError::Connect(error.to_string()),
        _ => TransportError::Disconnected(error.to_string()),
    }
}

/// The host's lease port: provider lanes name their `LlmApiKeyProvider` and
/// the slot's candidate list already arrives rotated — the head of each
/// resolution is the next authorized key, so retry attempts ride successive
/// keys exactly as the previous per-candidate loop did.
struct GatewayLeaseResolver {
    credentials: Arc<GatewayCredentialSlot>,
    kinds: Arc<HashMap<String, LlmApiKeyProvider>>,
}

#[async_trait::async_trait]
impl CredentialResolver for GatewayLeaseResolver {
    async fn resolve(&self, scope: CredentialScope<'_>) -> Result<Credential, a4mp::Error> {
        if matches!(scope.auth, AuthKind::None) {
            return Ok(Credential::None);
        }
        let kind = self
            .kinds
            .get(scope.target.provider.as_str())
            .copied()
            .ok_or_else(|| {
                a4mp::Error::Credential("provider has no leased credential kind".into())
            })?;
        let key = self
            .credentials
            .resolve_candidates(kind)
            .map_err(|_| a4mp::Error::Credential("credential_lease_unavailable".into()))?
            .into_iter()
            .next()
            .ok_or_else(|| a4mp::Error::Credential("credential_lease_unavailable".into()))?;
        let key = key
            .expose_utf8()
            .map_err(|_| a4mp::Error::Credential("credential_lease_unavailable".into()))?
            .to_owned();
        Ok(match scope.auth {
            AuthKind::ApiKey => Credential::ApiKey(Secret::new(key)),
            _ => Credential::Bearer {
                token: Secret::new(key),
                expires_at: None,
            },
        })
    }
}

// The adapter constructors answer `a4mp`'s registration signature, whose error
// type the provider crate owns; the constructors cannot box it.
#[allow(clippy::result_large_err)]
fn gateway_router(
    gateway: &CompiledGateway,
    credentials: &Arc<GatewayCredentialSlot>,
) -> Result<Router, GatewayTransportError> {
    let mut catalog = Catalog::new(gateway.provider_profiles().to_vec())
        .map_err(|_| GatewayTransportError::Gateway(GatewayError::InvalidConfig))?;
    let openai_ctor = Arc::new(
        |target: ServiceTarget,
         auth: AuthKind,
         transport: Arc<dyn Transport>,
         resolver: Arc<dyn CredentialResolver>,
         _effects: Option<Arc<dyn EffectHost>>| {
            a4mp::OpenAiAdapter::new(target, auth, transport, resolver)
                .map(|adapter| Arc::new(adapter) as Arc<dyn Adapter>)
        },
    );
    catalog.register_adapter(a4mp::Protocol::OpenAiChatCompletions, openai_ctor.clone());
    catalog.register_adapter(a4mp::Protocol::OpenAiResponses, openai_ctor);
    catalog.register_adapter(
        a4mp::Protocol::AnthropicMessages,
        Arc::new(
            |target: ServiceTarget,
             auth: AuthKind,
             transport: Arc<dyn Transport>,
             resolver: Arc<dyn CredentialResolver>,
             _effects: Option<Arc<dyn EffectHost>>| {
                a4mp::AnthropicAdapter::new(target, auth, transport, resolver)
                    .map(|adapter| Arc::new(adapter) as Arc<dyn Adapter>)
            },
        ),
    );
    let resolver: Arc<dyn CredentialResolver> = Arc::new(GatewayLeaseResolver {
        credentials: Arc::clone(credentials),
        kinds: Arc::new(gateway.credential_kinds().clone()),
    });
    Ok(Router::with_catalog(
        catalog,
        upstream_transport(),
        resolver,
    ))
}

// ---------------------------------------------------------------------------
// Model catalog
// ---------------------------------------------------------------------------

/// Fetch the current catalog from every provider whose credential is present in
/// the live lease. The upstream model object is preserved, while `id` is
/// namespaced with the provider lane so a later inference request is
/// unambiguous.
pub fn list_models(
    gateway: &CompiledGateway,
    credentials: &Arc<GatewayCredentialSlot>,
) -> Result<GatewayResponse, GatewayTransportError> {
    let _permit = Permit::acquire()?;
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
            Ok(candidates) => candidates.len(),
            Err(_) => {
                failed_providers += 1;
                first_failure.get_or_insert(CatalogFailure::Transport(
                    GatewayTransportError::CredentialUnavailable,
                ));
                continue;
            }
        };
        match fetch_provider_models(gateway, credentials, &provider, candidates, deadline) {
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
    Models(Vec<ModelInfo>),
    UpstreamError(GatewayResponse),
}

enum CatalogFailure {
    Transport(GatewayTransportError),
    Upstream(GatewayResponse),
}

fn project_provider_models(
    provider: &GatewayProvider,
    provider_models: Vec<ModelInfo>,
) -> Result<Vec<(String, Value)>, GatewayTransportError> {
    let mut projected = Vec::with_capacity(provider_models.len());
    let mut seen = BTreeSet::new();
    for info in provider_models {
        let Some(requested_id) = namespaced_model_id(&provider.id, &info.id) else {
            continue;
        };
        if !seen.insert(requested_id.clone()) {
            continue;
        }
        let mut model = json!({});
        let object = model.as_object_mut().unwrap();
        for (key, value) in info.extensions {
            object.insert(key, value);
        }
        object.insert("id".into(), Value::String(requested_id.clone()));
        object.insert("upstream_id".into(), Value::String(info.id));
        object.insert(
            "gateway_provider".into(),
            Value::String(provider.id.clone()),
        );
        object
            .entry("object")
            .or_insert_with(|| Value::String("model".into()));
        object.insert(
            "owned_by".into(),
            Value::String(info.owned_by.unwrap_or_else(|| provider.id.clone())),
        );
        projected.push((requested_id, model));
    }
    Ok(projected)
}

fn fetch_provider_models(
    gateway: &CompiledGateway,
    credentials: &Arc<GatewayCredentialSlot>,
    provider: &GatewayProvider,
    candidates: usize,
    deadline: Instant,
) -> Result<ProviderCatalog, GatewayTransportError> {
    if Instant::now() >= deadline {
        return Err(GatewayTransportError::TransportFailed);
    }
    let mut router = gateway_router(gateway, credentials)?;
    let route = gateway
        .provider_route(provider, "")
        .map_err(GatewayTransportError::Gateway)?;
    router
        .add_route(provider.id.clone(), route)
        .map_err(|_| GatewayTransportError::Gateway(GatewayError::InvalidConfig))?;
    let bound = router
        .resolve(&provider.id)
        .map_err(|_| GatewayTransportError::Gateway(GatewayError::InvalidConfig))?;
    for attempt in 0..candidates.max(1) {
        if Instant::now() >= deadline {
            return Err(GatewayTransportError::TransportFailed);
        }
        match block_on(bound.adapter.models(&RequestControl::default())) {
            Ok(models) => return Ok(ProviderCatalog::Models(models)),
            Err(error) if attempt + 1 < candidates && retryable_failure(&error) => continue,
            Err(error) => return dispatch_or_provider_models(error),
        }
    }
    Err(GatewayTransportError::CredentialUnavailable)
}

fn dispatch_or_provider_models(
    error: a4mp::Error,
) -> Result<ProviderCatalog, GatewayTransportError> {
    match error {
        a4mp::Error::Provider {
            status: Some(status),
            message,
            ..
        } => Ok(ProviderCatalog::UpstreamError(model_catalog_error(
            status, &message,
        ))),
        error => Err(dispatch_error(&error)),
    }
}

fn model_catalog_error(status: u16, message: &str) -> GatewayResponse {
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

// ---------------------------------------------------------------------------
// Forward path
// ---------------------------------------------------------------------------

/// Routes, canonically decodes and executes one model request through the
/// fairliny router. Credential lookup happens only after the closed route and
/// endpoint have been accepted.
pub fn exchange(
    gateway: &CompiledGateway,
    path: &str,
    body: &[u8],
    credentials: &Arc<GatewayCredentialSlot>,
) -> Result<GatewayResponse, GatewayTransportError> {
    match exchange_inner(gateway, path, body, credentials, None)? {
        GatewayExchange::Buffered(response) => Ok(response),
        GatewayExchange::Streamed => Err(GatewayTransportError::TransportFailed),
    }
}

pub fn exchange_to_sink(
    gateway: &CompiledGateway,
    path: &str,
    body: &[u8],
    credentials: &Arc<GatewayCredentialSlot>,
    sink: &mut dyn GatewayStreamSink,
) -> Result<GatewayExchange, GatewayTransportError> {
    exchange_inner(gateway, path, body, credentials, Some(sink))
}

fn exchange_inner(
    gateway: &CompiledGateway,
    path: &str,
    body: &[u8],
    credentials: &Arc<GatewayCredentialSlot>,
    mut sink: Option<&mut dyn GatewayStreamSink>,
) -> Result<GatewayExchange, GatewayTransportError> {
    let prepared = gateway
        .prepare(path, body)
        .map_err(GatewayTransportError::Gateway)?;
    let candidates = credentials
        .resolve_candidates(prepared.credential_provider)
        .map_err(|_| GatewayTransportError::CredentialUnavailable)?
        .len()
        .max(1);
    let _permit = Permit::acquire()?;
    let mut router = gateway_router(gateway, credentials)?;
    router
        .add_route(prepared.requested_model.clone(), prepared.route.clone())
        .map_err(|_| GatewayTransportError::Gateway(GatewayError::InvalidConfig))?;
    let bound = router
        .resolve(&prepared.requested_model)
        .map_err(|_| GatewayTransportError::Gateway(GatewayError::InvalidConfig))?;
    for attempt in 0..candidates {
        let control = RequestControl::default();
        let request = prepared.request.clone();
        if prepared.stream {
            match block_on(bound.adapter.chat_stream(request, &control)) {
                Ok(stream) => match sink.as_deref_mut() {
                    Some(sink) => {
                        return pump_stream(gateway, &prepared, stream, sink);
                    }
                    // `exchange` keeps its buffered contract for streamed
                    // requests too: the SSE body is collected whole.
                    None => return collect_stream(gateway, &prepared, stream),
                },
                Err(error) if attempt + 1 < candidates && retryable_failure(&error) => continue,
                Err(error) => return dispatch_or_provider_exchange(error),
            }
        }
        match block_on(bound.adapter.chat(request, &control)) {
            Ok(output) => {
                return gateway
                    .finish(&prepared, &output)
                    .map(GatewayExchange::Buffered)
                    .map_err(GatewayTransportError::Gateway);
            }
            Err(error) if attempt + 1 < candidates && retryable_failure(&error) => continue,
            Err(error) => return dispatch_or_provider_exchange(error),
        }
    }
    Err(GatewayTransportError::CredentialUnavailable)
}

fn dispatch_or_provider_exchange(
    error: a4mp::Error,
) -> Result<GatewayExchange, GatewayTransportError> {
    match error {
        a4mp::Error::Provider {
            status: Some(status),
            message,
            ..
        } => Ok(GatewayExchange::Buffered(normalize_upstream_error(
            status, &message,
        ))),
        error => Err(dispatch_error(&error)),
    }
}

/// One upstream failure while another leased candidate remains means the next
/// attempt rides the next authorized key — the slot's rotated candidate list
/// serves a different head per resolution. a4mp's own retry-safety verdict is
/// authoritative: only failures the provider provably rejected before
/// accepting work (auth/rate-limit refusals, connect failures) may fail over;
/// a mid-flight failure or a 5xx may already have billed and never repeats.
fn retryable_failure(error: &a4mp::Error) -> bool {
    match error {
        a4mp::Error::Provider {
            status: Some(status),
            retry: a4mp::RetrySafety::Safe,
            ..
        } => matches!(status, 401 | 403 | 429),
        a4mp::Error::Transport(TransportError::Connect(_)) => true,
        _ => false,
    }
}

fn dispatch_error(error: &a4mp::Error) -> GatewayTransportError {
    match error {
        a4mp::Error::Transport(TransportError::Io(message)) if message == RESPONSE_TOO_LARGE => {
            GatewayTransportError::ResponseTooLarge
        }
        a4mp::Error::Credential(_) => GatewayTransportError::CredentialUnavailable,
        a4mp::Error::Conversion { .. } | a4mp::Error::Unsupported(_) => {
            GatewayTransportError::Gateway(GatewayError::UnsupportedConversion)
        }
        a4mp::Error::UpstreamProtocol(_) => {
            GatewayTransportError::Gateway(GatewayError::InvalidUpstreamResponse)
        }
        a4mp::Error::Protocol(_) => GatewayTransportError::Gateway(GatewayError::InvalidRequest),
        _ => GatewayTransportError::TransportFailed,
    }
}

fn pump_stream(
    gateway: &CompiledGateway,
    prepared: &PreparedGatewayRequest,
    mut stream: a4mp::DynStream<'static, a4mp::StreamEvent>,
    sink: &mut dyn GatewayStreamSink,
) -> Result<GatewayExchange, GatewayTransportError> {
    let mut relay = ClientStreamRelay::new(prepared);
    let mut coalescer = SseFrameCoalescer::new();
    sink.begin(200, "text/event-stream")
        .map_err(|_| GatewayTransportError::TransportFailed)?;
    coalescer
        .push(&relay.preamble(), sink)
        .map_err(|_| GatewayTransportError::TransportFailed)?;
    while let Some(item) = block_on(stream.next()) {
        let event = item.map_err(|error| dispatch_error(&error))?;
        let bytes = relay.push(&event);
        if !bytes.is_empty() {
            coalescer
                .push(&bytes, sink)
                .map_err(|_| GatewayTransportError::TransportFailed)?;
        }
    }
    let tail = relay.finish();
    if !tail.is_empty() {
        coalescer
            .push(&tail, sink)
            .map_err(|_| GatewayTransportError::TransportFailed)?;
    }
    coalescer
        .finish(sink)
        .map_err(|_| GatewayTransportError::TransportFailed)?;
    if let Some(history) = relay.take_history() {
        gateway
            .remember_stream_response(prepared, history)
            .map_err(GatewayTransportError::Gateway)?;
    }
    Ok(GatewayExchange::Streamed)
}

fn collect_stream(
    gateway: &CompiledGateway,
    prepared: &PreparedGatewayRequest,
    mut stream: a4mp::DynStream<'static, a4mp::StreamEvent>,
) -> Result<GatewayExchange, GatewayTransportError> {
    let mut relay = ClientStreamRelay::new(prepared);
    let mut body = relay.preamble();
    while let Some(item) = block_on(stream.next()) {
        let event = item.map_err(|error| dispatch_error(&error))?;
        body.extend_from_slice(&relay.push(&event));
        if body.len() > MAX_GATEWAY_BODY_BYTES {
            return Err(GatewayTransportError::ResponseTooLarge);
        }
    }
    body.extend_from_slice(&relay.finish());
    if let Some(history) = relay.take_history() {
        gateway
            .remember_stream_response(prepared, history)
            .map_err(GatewayTransportError::Gateway)?;
    }
    if body.len() > MAX_GATEWAY_BODY_BYTES {
        return Err(GatewayTransportError::ResponseTooLarge);
    }
    Ok(GatewayExchange::Buffered(GatewayResponse {
        status: 200,
        content_type: "text/event-stream",
        body,
    }))
}

fn normalize_upstream_error(status: u16, message: &str) -> GatewayResponse {
    let message: String = message.chars().take(1024).collect();
    GatewayResponse {
        status,
        content_type: "application/json",
        body: serde_json::to_vec(&json!({"error":{"message":message,"type":"upstream_error","code":status,"param":null}})).unwrap_or_default(),
    }
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
        UpstreamProtocol,
    };
    use anyhow::Result;
    use credential_lease::SecretBytes;
    use std::io::Write as _;
    use std::net::TcpListener;

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

    fn fixture_slot(secrets: &[&str]) -> Arc<GatewayCredentialSlot> {
        fixture_slot_for(LlmApiKeyProvider::Kimi, secrets)
    }

    fn fixture_slot_for(
        provider: LlmApiKeyProvider,
        secrets: &[&str],
    ) -> Arc<GatewayCredentialSlot> {
        fixture_slot_for_providers(
            secrets
                .iter()
                .map(|secret| (provider, *secret))
                .collect::<Vec<_>>(),
        )
    }

    fn fixture_slot_for_providers(
        entries: Vec<(LlmApiKeyProvider, &str)>,
    ) -> Arc<GatewayCredentialSlot> {
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
        Arc::new(GatewayCredentialSlot::new(
            GatewayCredentialLease::new(
                credentials,
                GatewayCredentialLeaseDays::Seven.duration(),
                epoch.clone(),
                Arc::new(StaticEpochSource(epoch)),
            )
            .unwrap(),
        ))
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
    fn model_catalog_forwards_the_live_provider_response_with_namespaced_ids() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_one_request(&mut stream);
            let request = String::from_utf8_lossy(&request);
            assert!(request.starts_with("GET /v1/models HTTP/1.1"), "{request}");
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
        let slot = Arc::new(GatewayCredentialSlot::disconnected());

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

        let gateway = fixture_gateway(address);
        let slot = fixture_slot(&["fixture-first-key", "fixture-second-key"]);

        let response = exchange(
            &gateway,
            "/v1/chat/completions",
            br#"{"model":"requested","messages":[{"role":"user","content":"hello"}]}"#,
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

        let gateway = fixture_gateway(address);
        let slot = fixture_slot(&["fixture-stream-key"]);
        let (event_sender, event_receiver) = std::sync::mpsc::channel();
        let exchange = std::thread::spawn(move || {
            let mut sink = ChannelSink(event_sender);
            exchange_to_sink(
                &gateway,
                "/v1/chat/completions",
                br#"{"model":"requested","stream":true,"messages":[]}"#,
                &slot,
                &mut sink,
            )
        });

        // The re-encoded stream opens with a role chunk; content frames must
        // still arrive before the upstream body completes.
        let mut received = Vec::new();
        while !String::from_utf8_lossy(&received).contains("first") {
            received.extend_from_slice(
                &event_receiver
                    .recv_timeout(Duration::from_secs(5))
                    .expect("first SSE event before upstream completion"),
            );
        }
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
                &slot,
                &mut sink,
            )
        });

        let mut received = Vec::new();
        let mut writes = Vec::new();
        while let Ok(chunk) = event_receiver.recv_timeout(Duration::from_secs(5)) {
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

        let text = String::from_utf8(received.clone()).unwrap();
        for index in 0..40 {
            assert!(
                text.contains(&format!("\"content\":\"chunk-{index}\"")),
                "missing chunk {index}"
            );
        }
        assert!(text.ends_with("data: [DONE]\n\n"));
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
    fn truncated_upstream_frame_fails_closed() {
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
                &slot,
                &mut sink,
            )
        });

        match exchange.join().unwrap() {
            Err(error) => {
                // Nothing reaches the client ahead of the failure.
                assert!(matches!(
                    error,
                    GatewayTransportError::Gateway(_) | GatewayTransportError::TransportFailed
                ));
            }
            other => panic!("truncated stream must fail closed: {other:?}"),
        }
        while let Ok(chunk) = event_receiver.recv_timeout(Duration::from_millis(200)) {
            assert!(
                chunk.iter().all(u8::is_ascii_whitespace),
                "a truncated upstream must not emit partial client frames"
            );
        }
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
                &slot,
                &mut sink,
            )
        });

        let mut received = Vec::new();
        while let Ok(chunk) = event_receiver.recv_timeout(Duration::from_secs(5)) {
            received.extend_from_slice(&chunk);
        }
        match exchange.join().unwrap() {
            Ok(GatewayExchange::Streamed) => {}
            other => panic!("stream did not complete: {other:?}"),
        }
        server.join().unwrap();

        // The response id the client saw on `response.created` is the one the
        // follow-up continuation names.
        let text = String::from_utf8(received).unwrap();
        let created = text
            .split("\n\n")
            .find_map(|frame| {
                let data = frame.lines().find_map(|line| line.strip_prefix("data: "))?;
                let value: Value = serde_json::from_str(data).ok()?;
                (value["type"] == "response.created")
                    .then(|| {
                        value
                            .pointer("/response/id")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .flatten()
            })
            .expect("response.created carried the minted id");

        let follow_up = gateway
            .prepare(
                "/v1/responses",
                serde_json::to_vec(&serde_json::json!({
                    "model":"requested","input":"second","previous_response_id":created
                }))
                .unwrap()
                .as_slice(),
            )
            .unwrap();
        let assistant = follow_up
            .request
            .messages
            .iter()
            .find(|message| message.role == a4mp::Role::Assistant)
            .expect("recorded assistant turn replayed");
        assert!(assistant.content.iter().any(|part| matches!(
            part,
            a4mp::ContentPart::Text { text } if text == "streamed answer"
        )));
        assert!(assistant.content.iter().any(|part| matches!(
            part,
            a4mp::ContentPart::Reasoning(block)
                if block.opaque["reasoning_content"] == "fixture reasoning"
        )));
        assert!(assistant.content.iter().any(|part| matches!(
            part,
            a4mp::ContentPart::ToolCall(call)
                if call.id == "call-1" && call.name == "lookup"
                    && call.arguments == serde_json::json!({"q":"fixture"})
        )));
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
        let credentials = Arc::new(crate::credential::GatewayCredentialSlot::disconnected());
        let request = serde_json::json!({
            "model": "deepseek:deepseek-v4-flash",
            "messages": [{"role": "user", "content": "synthetic"}]
        });
        let error = exchange(
            &gateway,
            "/v1/chat/completions",
            &serde_json::to_vec(&request).unwrap(),
            &credentials,
        )
        .unwrap_err();
        assert_eq!(error, GatewayTransportError::CredentialUnavailable);
    }
}
