//! Independently owned authenticated loopback Streamable HTTP MCP service.

use crate::application::{
    CallerContext, MAX_MCP_FRAME_BYTES, SubagentMcpApplication, production_application,
    server_definition,
};
use crate::{
    McpMessage, McpRequestId, McpServerEngine, McpSessionState, decode_http_body, encode_http_body,
};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const DISCOVERY_SCHEMA: &str = "licoup.subagent-mcp.discovery.v1";
const MAX_HTTP_HEADER_BYTES: usize = 16 * 1024;
const MAX_HTTP_CONNECTIONS: usize = 32;
const MAX_SESSIONS: usize = 64;
const MAX_TOOL_WORKERS: usize = 8;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DiscoveryDocument {
    pub(crate) schema_version: String,
    pub(crate) endpoint: String,
    pub(crate) generation: String,
    pub(crate) tokens: HashMap<String, String>,
    pub(crate) control_token: String,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ConnectorDiscovery {
    pub(crate) endpoint: String,
    bearer_token: String,
}

pub struct SubagentMcpSupervisor {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    discovery_path: PathBuf,
    pub(crate) generation: String,
    address: SocketAddr,
    probe_identity: ConnectorDiscovery,
}

impl SubagentMcpSupervisor {
    pub fn start() -> Result<Self> {
        let application =
            production_application().map_err(|_| anyhow!("subagent_mcp_unavailable"))?;
        Self::start_with_application(application)
    }

    fn start_with_application(application: SubagentMcpApplication) -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        if !address.ip().is_loopback() {
            return Err(anyhow!("subagent_mcp_loopback_required"));
        }
        // Complete every fallible in-process construction before publishing
        // discovery. After publication, the only fallible step is thread
        // creation, whose error path performs generation-bound cleanup.
        let engine = McpServerEngine::new(server_definition(), application.clone())?;
        let generation = uuid::Uuid::new_v4().simple().to_string();
        // The token map is the capability set made concrete: exactly the mesh
        // callers the adapter registry admits. Publishing it here also lets the
        // connector distinguish "this Agent has no mesh seat" from "the
        // service is unreachable", without consulting a list of its own.
        let control_token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let tokens = application
            .caller_providers()
            .into_iter()
            .map(|provider| {
                (
                    provider,
                    format!(
                        "{}{}",
                        uuid::Uuid::new_v4().simple(),
                        uuid::Uuid::new_v4().simple()
                    ),
                )
            })
            .collect::<HashMap<_, _>>();
        let discovery_path = discovery_path()?;
        write_discovery(
            &discovery_path,
            &DiscoveryDocument {
                schema_version: DISCOVERY_SCHEMA.to_owned(),
                endpoint: format!("http://{address}/mcp"),
                generation: generation.clone(),
                tokens: tokens.clone(),
                control_token: control_token.clone(),
            },
        )?;
        let probe_identity = ConnectorDiscovery {
            endpoint: format!("http://{address}/mcp"),
            bearer_token: tokens
                .keys()
                .min()
                .and_then(|provider| tokens.get(provider))
                .cloned()
                .ok_or_else(|| anyhow!("subagent_mcp_unavailable"))?,
        };
        let stop = Arc::new(AtomicBool::new(false));
        let service = Arc::new(ServiceState {
            engine,
            tokens,
            control_token,
            stop: Arc::clone(&stop),
            sessions: Mutex::new(HashMap::new()),
            active_connections: AtomicUsize::new(0),
            active_tool_calls: AtomicUsize::new(0),
            active_control_calls: AtomicUsize::new(0),
            address,
        });
        let thread_stop = Arc::clone(&stop);
        let thread = match thread::Builder::new()
            .name("subagent-mcp-http".to_owned())
            .spawn(move || serve(listener, service, thread_stop))
        {
            Ok(thread) => thread,
            Err(error) => {
                cleanup_discovery_generation(&discovery_path, &generation);
                return Err(error.into());
            }
        };
        Ok(Self {
            stop,
            thread: Some(thread),
            discovery_path,
            generation,
            address,
            probe_identity,
        })
    }

    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    pub fn healthy(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
            && self.address.ip().is_loopback()
    }

    /// Health is the real contract, not thread liveness: one authenticated
    /// initialize plus the exact ordered tool catalog. The probe identity
    /// never leaves the process and the probe session is closed afterwards.
    pub fn health_probe(&self) -> bool {
        let initialize = McpMessage::request(
            McpRequestId::from(1_i64),
            "initialize",
            Some(
                serde_json::json!({
                    "protocolVersion": crate::application::PROTOCOL_REVISION,
                    "capabilities": {},
                    "clientInfo": {"name": "licoup-supervisor-health", "version": "1"}
                })
                .as_object()
                .cloned()
                .unwrap_or_default(),
            ),
        );
        let Ok(initialize) = initialize else {
            return false;
        };
        let Ok(body) = encode_http_body(&initialize, MAX_MCP_FRAME_BYTES) else {
            return false;
        };
        let Ok((status, session, response)) = health_exchange(&self.probe_identity, None, &body)
        else {
            return false;
        };
        let session = session.filter(|session| !session.is_empty());
        if status != 200 {
            if let Some(session) = session.as_deref() {
                let _ = connector_close_session(&self.probe_identity, session);
            }
            return false;
        }
        let Some(session) = session else {
            return false;
        };
        let healthy = (|| {
            let negotiated = decode_http_body(&response, MAX_MCP_FRAME_BYTES)
                .ok()
                .map(|message| message.to_value());
            let valid_initialize = negotiated.as_ref().is_some_and(|value| {
                value
                    .pointer("/result/protocolVersion")
                    .and_then(Value::as_str)
                    == Some(crate::application::PROTOCOL_REVISION)
                    && value
                        .pointer("/result/serverInfo/name")
                        .and_then(Value::as_str)
                        == Some(crate::application::SERVER_NAME)
                    && value
                        .pointer("/result/serverInfo/version")
                        .and_then(Value::as_str)
                        == Some(crate::application::SERVER_VERSION)
            });
            if !valid_initialize {
                return false;
            }
            let Ok(list) = McpMessage::request(2_i64, "tools/list", Some(Map::new())) else {
                return false;
            };
            let Ok(body) = encode_http_body(&list, MAX_MCP_FRAME_BYTES) else {
                return false;
            };
            let listed = health_exchange(&self.probe_identity, Some(&session), &body)
                .ok()
                .and_then(|(status, returned, response)| {
                    (status == 200 && returned.as_deref() == Some(session.as_str()))
                        .then(|| decode_http_body(&response, MAX_MCP_FRAME_BYTES).ok())
                        .flatten()
                })
                .map(|message| message.to_value());
            listed.is_some_and(|value| {
                value
                    .pointer("/result/tools")
                    .and_then(Value::as_array)
                    .map(|tools| {
                        tools
                            .iter()
                            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                    })
                    .as_deref()
                    == Some(crate::application::TOOL_NAMES)
            })
        })();
        let closed = connector_close_session(&self.probe_identity, &session).is_ok();
        healthy && closed
    }
}

impl Drop for SubagentMcpSupervisor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(100));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        cleanup_discovery_generation(&self.discovery_path, &self.generation);
    }
}

struct ServiceState {
    engine: McpServerEngine<SubagentMcpApplication>,
    control_token: String,
    stop: Arc<AtomicBool>,
    pub(crate) tokens: HashMap<String, String>,
    sessions: Mutex<HashMap<String, ServerSession>>,
    active_connections: AtomicUsize,
    active_tool_calls: AtomicUsize,
    active_control_calls: AtomicUsize,
    address: SocketAddr,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct SessionCallerBinding {
    conversation_id: Option<String>,
    membership_id: Option<String>,
    parent_dispatch_id: Option<String>,
}

struct ServerSession {
    provider: String,
    state: Arc<McpSessionState>,
    caller: Mutex<SessionCallerBinding>,
}

fn merge_session_caller_binding(
    existing: &SessionCallerBinding,
    headers: &HashMap<String, String>,
) -> SessionCallerBinding {
    SessionCallerBinding {
        conversation_id: headers
            .get("x-licoup-conversation-id")
            .cloned()
            .or_else(|| existing.conversation_id.clone()),
        membership_id: headers
            .get("x-licoup-membership-id")
            .cloned()
            .or_else(|| existing.membership_id.clone()),
        parent_dispatch_id: headers
            .get("x-licoup-parent-dispatch-id")
            .cloned()
            .or_else(|| existing.parent_dispatch_id.clone()),
    }
}

fn serve(listener: TcpListener, service: Arc<ServiceState>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if !peer.ip().is_loopback()
                    || service
                        .active_connections
                        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                            (count < MAX_HTTP_CONNECTIONS).then_some(count + 1)
                        })
                        .is_err()
                {
                    continue;
                }
                let request_service = Arc::clone(&service);
                if thread::Builder::new()
                    .name("subagent-mcp-request".to_owned())
                    .spawn(move || {
                        let _permit = ConnectionPermit(&request_service.active_connections);
                        let _ = handle_connection(stream, &request_service);
                    })
                    .is_err()
                {
                    service.active_connections.fetch_sub(1, Ordering::AcqRel);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => thread::sleep(Duration::from_millis(10)),
        }
    }
    while service.active_connections.load(Ordering::Acquire) != 0 {
        thread::sleep(Duration::from_millis(10));
    }
}

struct ConnectionPermit<'a>(&'a AtomicUsize);

impl Drop for ConnectionPermit<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn handle_connection(mut stream: TcpStream, service: &ServiceState) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let request = match read_http_request(&mut stream) {
        Ok(request) => request,
        Err(_) => return write_http(&mut stream, 400, &[], b""),
    };
    // Admission happens before any effect: exact numeric loopback Host, no
    // browser Origin, then the bearer client identity. A browser-origin or
    // misdirected request never reaches token comparison or session state.
    let expected_host = service.address.to_string();
    if request.headers.get("host").map(String::as_str) != Some(expected_host.as_str()) {
        return write_http(&mut stream, 400, &[], b"");
    }
    if request.headers.contains_key("origin") {
        return write_http(&mut stream, 400, &[], b"");
    }
    let Some(token) = request
        .headers
        .get("authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return write_http(&mut stream, 401, &[], b"");
    };
    if request.path == "/control/stop" {
        if request.method != "POST" || !request.body.is_empty() {
            return write_http(&mut stream, 400, &[], b"");
        }
        if !constant_time_eq(token, &service.control_token) {
            return write_http(&mut stream, 401, &[], b"");
        }
        service.stop.store(true, Ordering::Release);
        return write_http(&mut stream, 202, &[], b"");
    }
    if [
        "x-licoup-conversation-id",
        "x-licoup-membership-id",
        "x-licoup-parent-dispatch-id",
    ]
    .into_iter()
    .any(|name| {
        request
            .headers
            .get(name)
            .is_some_and(|value| !valid_context_header(value))
    }) {
        return write_http(&mut stream, 400, &[], b"");
    }
    let Some(provider) = service
        .tokens
        .iter()
        .find_map(|(provider, expected)| (constant_time_eq(token, expected)).then_some(provider))
    else {
        return write_http(&mut stream, 401, &[], b"");
    };
    if request.method == "GET" && request.path == "/health" {
        return write_http(&mut stream, 204, &[], b"");
    }
    if request.method == "DELETE" && request.path == "/mcp" {
        let Some(session_id) = request.headers.get("mcp-session-id") else {
            return write_http(&mut stream, 400, &[], b"");
        };
        let mut sessions = service
            .sessions
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let removed = sessions
            .get(session_id)
            .is_some_and(|session| session.provider == *provider)
            && sessions.remove(session_id).is_some();
        return write_http(&mut stream, if removed { 204 } else { 404 }, &[], b"");
    }
    if request.method != "POST" || request.path != "/mcp" {
        return write_http(&mut stream, 404, &[], b"");
    }
    if request.headers.get("content-type").map(String::as_str) != Some("application/json") {
        return write_http(&mut stream, 415, &[], b"");
    }
    let message = match decode_http_body(&request.body, MAX_MCP_FRAME_BYTES) {
        Ok(message) => message,
        Err(_) => return write_http(&mut stream, 400, &[], b""),
    };
    let initialize =
        matches!(&message, McpMessage::Request { method, .. } if method == "initialize");
    // The protocol revision is admitted before effects: a present header must
    // be one of the server's explicit revisions, and every post-initialize
    // request must carry the revision negotiated for that session.
    let request_protocol_revision = request
        .headers
        .get("mcp-protocol-version")
        .map(String::as_str);
    match request_protocol_revision {
        Some(revision)
            if service
                .engine
                .definition()
                .supports_protocol_revision(revision) => {}
        Some(_) => return write_http(&mut stream, 400, &[], b""),
        None if initialize => {}
        None => return write_http(&mut stream, 400, &[], b""),
    }
    let requested_session = request.headers.get("mcp-session-id").cloned();
    let (session_id, session, binding) = if initialize && requested_session.is_none() {
        let mut sessions = service
            .sessions
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if sessions.len() >= MAX_SESSIONS {
            return write_http(&mut stream, 503, &[], b"");
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let session = Arc::new(McpSessionState::default());
        let binding =
            merge_session_caller_binding(&SessionCallerBinding::default(), &request.headers);
        sessions.insert(
            id.clone(),
            ServerSession {
                provider: provider.clone(),
                state: Arc::clone(&session),
                caller: Mutex::new(binding.clone()),
            },
        );
        (id, session, binding)
    } else {
        let Some(id) = requested_session else {
            return write_http(&mut stream, 400, &[], b"");
        };
        let sessions = service
            .sessions
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let Some(stored) = sessions
            .get(&id)
            .filter(|session| session.provider == *provider)
        else {
            return write_http(&mut stream, 404, &[], b"");
        };
        let session = Arc::clone(&stored.state);
        let binding = {
            let mut guard = stored
                .caller
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            *guard = merge_session_caller_binding(&guard, &request.headers);
            guard.clone()
        };
        (id, session, binding)
    };
    if !initialize && session.protocol_revision() != request_protocol_revision {
        return write_http(&mut stream, 400, &[], b"");
    }
    // The bounded tool-effect permit is claimed only after the request has
    // proven its identity, revision, and session, so invalid calls can never
    // occupy an effect slot.
    let _tool_permit = if matches!(&message, McpMessage::Request { method, .. } if method == "tools/call")
    {
        let cancellation = matches!(&message, McpMessage::Request { params: Some(params), .. }
            if params.get("name").and_then(Value::as_str) == Some("lico_subagent_cancel"));
        let (counter, limit) = if cancellation {
            (&service.active_control_calls, 1)
        } else {
            (&service.active_tool_calls, MAX_TOOL_WORKERS)
        };
        match ToolPermit::acquire(counter, limit) {
            Some(permit) => Some(permit),
            None => return write_http(&mut stream, 503, &[], b""),
        }
    } else {
        None
    };
    let caller = CallerContext {
        provider_id: provider.clone(),
        conversation_id: binding.conversation_id,
        membership_id: binding.membership_id,
        parent_dispatch_id: binding.parent_dispatch_id,
        authenticated: true,
    };
    match service.engine.handle(&session, &caller, message) {
        Some(response) => {
            let body = encode_http_body(&response, MAX_MCP_FRAME_BYTES)?;
            let response_protocol_revision = session
                .protocol_revision()
                .unwrap_or(service.engine.definition().protocol_revision);
            write_http(
                &mut stream,
                200,
                &[
                    ("content-type", "application/json"),
                    ("mcp-session-id", session_id.as_str()),
                    ("mcp-protocol-version", response_protocol_revision),
                ],
                &body,
            )
        }
        None => write_http(
            &mut stream,
            202,
            &[("mcp-session-id", session_id.as_str())],
            b"",
        ),
    }
}

struct ToolPermit<'a>(&'a AtomicUsize);

impl<'a> ToolPermit<'a> {
    fn acquire(counter: &'a AtomicUsize, limit: usize) -> Option<Self> {
        counter
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < limit).then_some(count + 1)
            })
            .ok()
            .map(|_| Self(counter))
    }
}

impl Drop for ToolPermit<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest> {
    let mut bytes = Vec::with_capacity(1024);
    let header_end = loop {
        if bytes.len() >= MAX_HTTP_HEADER_BYTES {
            return Err(anyhow!("http_headers_too_large"));
        }
        let mut buffer = [0_u8; 1024];
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            return Err(anyhow!("http_request_incomplete"));
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let header_end = index + 4;
            if header_end > MAX_HTTP_HEADER_BYTES {
                return Err(anyhow!("http_headers_too_large"));
            }
            break header_end;
        }
    };
    let head = std::str::from_utf8(&bytes[..header_end - 4])?;
    let mut lines = head.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| anyhow!("http_request_invalid"))?;
    let mut request_parts = request_line.split(' ');
    let method = request_parts.next().unwrap_or_default().to_owned();
    let path = request_parts.next().unwrap_or_default().to_owned();
    if request_parts.next() != Some("HTTP/1.1") || request_parts.next().is_some() {
        return Err(anyhow!("http_request_invalid"));
    }
    let mut headers = HashMap::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| anyhow!("http_header_invalid"))?;
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() || headers.insert(name, value.trim().to_owned()).is_some() {
            return Err(anyhow!("http_header_invalid"));
        }
    }
    if headers.contains_key("transfer-encoding") {
        return Err(anyhow!("http_chunked_unsupported"));
    }
    let content_length = headers
        .get("content-length")
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(0);
    if content_length > MAX_MCP_FRAME_BYTES {
        return Err(anyhow!("http_body_too_large"));
    }
    let mut body = bytes[header_end..].to_vec();
    while body.len() < content_length {
        let mut buffer = [0_u8; 4096];
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            return Err(anyhow!("http_body_incomplete"));
        }
        body.extend_from_slice(&buffer[..count]);
        if body.len() > content_length {
            return Err(anyhow!("http_body_overrun"));
        }
    }
    if body.len() != content_length {
        return Err(anyhow!("http_body_invalid"));
    }
    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

fn write_http(
    stream: &mut TcpStream,
    status: u16,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        415 => "Unsupported Media Type",
        503 => "Service Unavailable",
        _ => "Error",
    };
    write!(stream, "HTTP/1.1 {status} {reason}\r\n")?;
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    write!(
        stream,
        "content-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

/// Owner path for publishing discovery. Constructing it creates and hardens the
/// state root, so it belongs to the supervisor that owns that state — never to a
/// reader.
fn discovery_path() -> Result<PathBuf> {
    let state_root = crate::private_state::portable_data_dir()?.join("client-state");
    let root = state_root.join("subagent-mcp");
    crate::private_state::ensure_private_dir(&state_root)?;
    crate::private_state::ensure_private_dir(&root)?;
    Ok(root.join("discovery.json"))
}

/// Reader path for discovery. Resolves lexically and never creates the state
/// root, so reading is side-effect free even when nothing has been published
/// yet. A refused connector must be able to report why without leaving the
/// directories it was refused before creating.
pub(crate) fn discovery_path_read_only() -> Result<PathBuf> {
    Ok(crate::private_state::portable_data_dir_read_only()?
        .join("client-state")
        .join("subagent-mcp")
        .join("discovery.json"))
}

fn write_discovery(path: &Path, document: &DiscoveryDocument) -> Result<()> {
    let text = serde_json::to_string(document)?;
    crate::private_state::atomic_write_private_text_bounded(path, &text, 16 * 1024)
}

pub(crate) fn read_discovery(path: &Path) -> Result<DiscoveryDocument> {
    let text = crate::private_state::read_existing_private_text_bounded(path, 16 * 1024)?
        .ok_or_else(|| anyhow!("subagent_mcp_discovery_unavailable"))?;
    let document: DiscoveryDocument = serde_json::from_str(&text)?;
    if !valid_discovery_document(&document) {
        return Err(anyhow!("subagent_mcp_discovery_invalid"));
    }
    Ok(document)
}

fn valid_discovery_document(document: &DiscoveryDocument) -> bool {
    let port = document
        .endpoint
        .strip_prefix("http://127.0.0.1:")
        .and_then(|value| value.strip_suffix("/mcp"));
    let valid_port = port.is_some_and(|value| {
        !value.is_empty()
            && value.bytes().all(|byte| byte.is_ascii_digit())
            && value
                .parse::<u16>()
                .ok()
                .is_some_and(|port| port > 0 && port.to_string() == value)
    });
    document.schema_version == DISCOVERY_SCHEMA
        && valid_port
        && lowercase_hex(&document.generation, 32)
        && lowercase_hex(&document.control_token, 64)
        && !document.tokens.is_empty()
        && document
            .tokens
            .iter()
            .all(|(provider, token)| valid_caller_id(provider) && lowercase_hex(token, 64))
}

/// A published caller id is an opaque Agent identifier. The set is deliberately
/// not compared against a known list here: the token map *is* the capability
/// set, and the reader must be able to tell "no seat for this Agent" from
/// "unreadable discovery".
fn valid_caller_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

fn lowercase_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn cleanup_discovery_generation(path: &Path, generation: &str) {
    let owned = crate::private_state::read_existing_private_text_bounded(path, 16 * 1024)
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| {
            value
                .get("generation")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|observed| observed == generation);
    if owned {
        let _ = fs::remove_file(path);
    }
}

/// Why a connector could not obtain its authenticated loopback identity.
///
/// The two causes need different user actions, so they are never collapsed into
/// one opaque failure: an unreachable desktop-owned service is an installation
/// or lifecycle problem, while an Agent without a mesh seat is simply not
/// supported and will not become supported by retrying.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectorDiscoveryError {
    Unavailable,
    /// The document is valid but admits no seat for this caller. The payload is
    /// the caller set it does admit, in provider-id order.
    CallerNotSupported(Vec<String>),
}

impl ConnectorDiscoveryError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Unavailable => "subagent_mcp_discovery_unavailable",
            Self::CallerNotSupported(_) => "subagent_mcp_caller_unsupported",
        }
    }
}

impl std::fmt::Display for ConnectorDiscoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

/// The callers the currently published service admits, in provider-id order.
/// Diagnostics only: absent, stale, or unreadable discovery yields an empty
/// list rather than an error, because it must never change an exit path.
pub fn published_callers() -> Vec<String> {
    discovery_path_read_only()
        .ok()
        .and_then(|path| read_discovery(&path).ok())
        .map(|document| {
            let mut providers = document.tokens.into_keys().collect::<Vec<_>>();
            providers.sort();
            providers
        })
        .unwrap_or_default()
}

pub fn load_connector_discovery(
    provider: &str,
) -> Result<ConnectorDiscovery, ConnectorDiscoveryError> {
    // A connector only ever reads discovery; the supervisor owns creating it.
    let path = discovery_path_read_only().map_err(|_| ConnectorDiscoveryError::Unavailable)?;
    let document = read_discovery(&path).map_err(|_| ConnectorDiscoveryError::Unavailable)?;
    // Membership is decided by the published capability set, not by a list of
    // known names compiled into this binary.
    let Some(token) = document.tokens.get(provider).cloned() else {
        let mut supported = document.tokens.into_keys().collect::<Vec<_>>();
        supported.sort();
        return Err(ConnectorDiscoveryError::CallerNotSupported(supported));
    };
    Ok(ConnectorDiscovery {
        endpoint: document.endpoint,
        bearer_token: token,
    })
}

pub fn connector_exchange(
    discovery: &ConnectorDiscovery,
    session_id: Option<&str>,
    protocol_revision: &str,
    body: &[u8],
) -> Result<(u16, Option<String>, Vec<u8>)> {
    if !crate::application::server_definition().supports_protocol_revision(protocol_revision) {
        return Err(anyhow!("subagent_mcp_protocol_revision_invalid"));
    }
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(2))
        .build();
    let mut request = agent
        .post(&discovery.endpoint)
        .set(
            "authorization",
            &format!("Bearer {}", discovery.bearer_token),
        )
        .set("content-type", "application/json")
        .set("accept", "application/json")
        .set("mcp-protocol-version", protocol_revision);
    for (header, variable) in [
        ("x-licoup-conversation-id", "LICOUP_MCP_CONVERSATION_ID"),
        ("x-licoup-membership-id", "LICOUP_MCP_MEMBERSHIP_ID"),
        (
            "x-licoup-parent-dispatch-id",
            "LICOUP_MCP_PARENT_DISPATCH_ID",
        ),
    ] {
        if let Ok(value) = std::env::var(variable)
            && valid_context_header(&value)
        {
            request = request.set(header, &value);
        }
    }
    if let Some(session_id) = session_id {
        request = request.set("mcp-session-id", session_id);
    }
    let response = match request.send_bytes(body) {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(_) => return Err(anyhow!("subagent_mcp_connector_transport_failed")),
    };
    let status = response.status();
    let session = response.header("mcp-session-id").map(str::to_owned);
    let mut body = Vec::new();
    response
        .into_reader()
        .take((MAX_MCP_FRAME_BYTES + 1) as u64)
        .read_to_end(&mut body)?;
    if body.len() > MAX_MCP_FRAME_BYTES {
        return Err(anyhow!("subagent_mcp_connector_response_too_large"));
    }
    Ok((status, session, body))
}

pub fn connector_close_session(discovery: &ConnectorDiscovery, session_id: &str) -> Result<()> {
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(2))
        .timeout_read(Duration::from_secs(3))
        .timeout_write(Duration::from_secs(3))
        .build();
    let response = agent
        .delete(&discovery.endpoint)
        .set(
            "authorization",
            &format!("Bearer {}", discovery.bearer_token),
        )
        .set("mcp-session-id", session_id)
        .call();
    match response {
        Ok(response) if response.status() == 204 => Ok(()),
        _ => Err(anyhow!("subagent_mcp_connector_close_failed")),
    }
}

/// Bounded health exchange used only by the supervisor's own probe. Tight
/// deadlines keep the monitor responsive and never carry caller context.
fn health_exchange(
    discovery: &ConnectorDiscovery,
    session_id: Option<&str>,
    body: &[u8],
) -> Result<(u16, Option<String>, Vec<u8>)> {
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(1))
        .timeout_read(Duration::from_secs(2))
        .timeout_write(Duration::from_secs(2))
        .build();
    let mut request = agent
        .post(&discovery.endpoint)
        .set(
            "authorization",
            &format!("Bearer {}", discovery.bearer_token),
        )
        .set("content-type", "application/json")
        .set("accept", "application/json")
        .set(
            "mcp-protocol-version",
            crate::application::PROTOCOL_REVISION,
        );
    if let Some(session_id) = session_id {
        request = request.set("mcp-session-id", session_id);
    }
    let response = match request.send_bytes(body) {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(_) => return Err(anyhow!("subagent_mcp_health_transport_failed")),
    };
    let status = response.status();
    let session = response.header("mcp-session-id").map(str::to_owned);
    let mut body = Vec::new();
    response
        .into_reader()
        .take((MAX_MCP_FRAME_BYTES + 1) as u64)
        .read_to_end(&mut body)?;
    if body.len() > MAX_MCP_FRAME_BYTES {
        return Err(anyhow!("subagent_mcp_health_response_too_large"));
    }
    Ok((status, session, body))
}

fn valid_context_header(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_compare_without_prefix_acceptance() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abc0"));
    }

    #[test]
    fn caller_context_headers_are_closed_identifiers() {
        assert!(valid_context_header("membership:fixture-1"));
        assert!(!valid_context_header("../private"));
        assert!(!valid_context_header("line\nbreak"));
    }

    #[test]
    fn later_request_reuses_session_caller_binding() {
        let initial = SessionCallerBinding {
            conversation_id: Some("conversation:one".into()),
            membership_id: Some("membership:caller".into()),
            parent_dispatch_id: None,
        };
        let reused = merge_session_caller_binding(&initial, &HashMap::new());
        assert_eq!(reused.conversation_id.as_deref(), Some("conversation:one"));
        assert_eq!(reused.membership_id.as_deref(), Some("membership:caller"));
        let mut headers = HashMap::new();
        headers.insert("x-licoup-conversation-id".into(), "conversation:two".into());
        let overridden = merge_session_caller_binding(&initial, &headers);
        assert_eq!(
            overridden.conversation_id.as_deref(),
            Some("conversation:two")
        );
        assert_eq!(
            overridden.membership_id.as_deref(),
            Some("membership:caller")
        );
    }

    #[test]
    fn discovery_admission_is_exact_and_lowercase() {
        let valid = || DiscoveryDocument {
            control_token: "c".repeat(64),
            schema_version: DISCOVERY_SCHEMA.to_owned(),
            endpoint: "http://127.0.0.1:34567/mcp".to_owned(),
            generation: "a".repeat(32),
            tokens: HashMap::from([
                ("antigravity".to_owned(), "b".repeat(64)),
                ("claude-code".to_owned(), "a".repeat(64)),
                ("codex".to_owned(), "c".repeat(64)),
                ("cursor".to_owned(), "d".repeat(64)),
            ]),
        };
        assert!(valid_discovery_document(&valid()));
        for endpoint in [
            "http://localhost:34567/mcp",
            "http://127.0.0.1:034567/mcp",
            "http://127.0.0.1:34567/mcp?extra=1",
            "http://127.0.0.1:34567/other",
        ] {
            let mut document = valid();
            document.endpoint = endpoint.to_owned();
            assert!(!valid_discovery_document(&document));
        }
        let mut uppercase = valid();
        uppercase.tokens.insert("codex".to_owned(), "A".repeat(64));
        assert!(!valid_discovery_document(&uppercase));
        let mut truncated = valid();
        truncated.tokens.insert("codex".to_owned(), "c".repeat(63));
        assert!(!valid_discovery_document(&truncated));
        let mut empty = valid();
        empty.tokens.clear();
        assert!(!valid_discovery_document(&empty));
        let mut malformed_id = valid();
        malformed_id
            .tokens
            .insert("Codex CLI".to_owned(), "e".repeat(64));
        assert!(!valid_discovery_document(&malformed_id));
    }

    /// The token map is the capability set, so a caller the reader does not
    /// recognise is admitted structurally. Deciding that an Agent has no mesh
    /// seat is the reader's job, not a reason to reject the whole document.
    #[test]
    fn discovery_admits_a_caller_set_the_reader_does_not_know() {
        let mut document = DiscoveryDocument {
            control_token: "c".repeat(64),
            schema_version: DISCOVERY_SCHEMA.to_owned(),
            endpoint: "http://127.0.0.1:34567/mcp".to_owned(),
            generation: "a".repeat(32),
            tokens: HashMap::from([("some-new-agent".to_owned(), "b".repeat(64))]),
        };
        assert!(valid_discovery_document(&document));
        assert!(valid_caller_id("some-new-agent"));
        assert!(!valid_caller_id("Some New Agent"));
        assert!(!valid_caller_id(""));

        document.tokens.clear();
        assert!(!valid_discovery_document(&document));
    }
}

/// Readiness observes transport only; no tool invocation or native probe runs.
pub fn connector_health(discovery: &ConnectorDiscovery) -> bool {
    let Some(endpoint) = discovery.endpoint.strip_suffix("/mcp") else {
        return false;
    };
    ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(2))
        .timeout_read(Duration::from_secs(2))
        .build()
        .get(&format!("{endpoint}/health"))
        .set(
            "authorization",
            &format!("Bearer {}", discovery.bearer_token),
        )
        .call()
        .is_ok_and(|response| response.status() == 204)
}
