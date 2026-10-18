//! Process-local, loopback-only HTTP front end for the LLM gateway.

use crate::http::transport::{
    GatewayExchange, GatewayStreamSink, GatewayTransportError, exchange_to_sink, list_models,
};
use crate::model::llm_gateway::{CompiledGateway, GatewayResponse, MAX_GATEWAY_BODY_BYTES};
use crate::usage::GatewayUsageRecorder;
use licoup_foundation::core::secret_bytes::SecretBytes;
use licoup_vault::api_key::GatewayCredentialSlot;
use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_HEADERS: usize = 128;
const MAX_REQUESTS_PER_CONNECTION: usize = 64;
const WORKERS: usize = 8;
const QUEUE_DEPTH: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatewayServerError {
    NonLoopbackListener,
    ListenerConfiguration,
    WorkerFailure,
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

/// Serves until `stop` is set. The caller owns configuration loading and the
/// listener lifetime; this function never edits host application files.
pub fn serve_loopback(
    listener: TcpListener,
    gateway: Arc<CompiledGateway>,
    stop: Arc<AtomicBool>,
    credentials: Arc<GatewayCredentialSlot>,
    client_token: Arc<SecretBytes>,
    usage: Arc<GatewayUsageRecorder>,
) -> Result<(), GatewayServerError> {
    let address = listener
        .local_addr()
        .map_err(|_| GatewayServerError::ListenerConfiguration)?;
    if !address.ip().is_loopback() {
        return Err(GatewayServerError::NonLoopbackListener);
    }
    listener
        .set_nonblocking(true)
        .map_err(|_| GatewayServerError::ListenerConfiguration)?;
    let (sender, receiver) = mpsc::sync_channel::<TcpStream>(QUEUE_DEPTH);
    let receiver = Arc::new(Mutex::new(receiver));
    let mut workers = Vec::with_capacity(WORKERS);
    for _ in 0..WORKERS {
        let receiver = Arc::clone(&receiver);
        let gateway = Arc::clone(&gateway);
        let credentials = Arc::clone(&credentials);
        let client_token = Arc::clone(&client_token);
        let usage = Arc::clone(&usage);
        workers.push(thread::spawn(move || {
            worker(receiver, gateway, credentials, client_token, usage)
        }));
    }
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((mut stream, peer)) => {
                if !peer.ip().is_loopback() {
                    let _ = write_error(&mut stream, 403, "loopback_client_required", false);
                    continue;
                }
                match sender.try_send(stream) {
                    Ok(()) => {}
                    Err(mpsc::TrySendError::Full(mut stream)) => {
                        let _ = write_error(&mut stream, 503, "gateway_busy", false);
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => {
                        return Err(GatewayServerError::WorkerFailure);
                    }
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return Err(GatewayServerError::ListenerConfiguration),
        }
    }
    drop(sender);
    for worker in workers {
        worker
            .join()
            .map_err(|_| GatewayServerError::WorkerFailure)?;
    }
    Ok(())
}

fn worker(
    receiver: Arc<Mutex<mpsc::Receiver<TcpStream>>>,
    gateway: Arc<CompiledGateway>,
    credentials: Arc<GatewayCredentialSlot>,
    client_token: Arc<SecretBytes>,
    usage: Arc<GatewayUsageRecorder>,
) {
    loop {
        let stream = match receiver.lock() {
            Ok(receiver) => receiver.recv(),
            Err(_) => return,
        };
        let Ok(mut stream) = stream else { return };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
        handle_connection(&mut stream, &gateway, &credentials, &client_token, &usage);
    }
}

fn handle_connection(
    stream: &mut TcpStream,
    gateway: &CompiledGateway,
    credentials: &GatewayCredentialSlot,
    client_token: &SecretBytes,
    usage: &GatewayUsageRecorder,
) {
    // One accepted socket serves at most 64 validated sequential requests.
    // Unused parser bytes carry over between requests so fragmented and
    // pipelined traffic needs no repeated accepts. Streamed SSE responses
    // always terminate the connection after the terminal frame.
    let mut carry = Vec::new();
    for served in 0..MAX_REQUESTS_PER_CONNECTION {
        let keep_alive = served + 1 < MAX_REQUESTS_PER_CONNECTION;
        let request = match read_request(stream, &mut carry) {
            Ok(Some(request)) => request,
            Ok(None) => return,
            Err(code) => {
                let _ = write_error(stream, code, "invalid_gateway_request", false);
                return;
            }
        };
        let client_close = request
            .headers
            .get("connection")
            .is_some_and(|value| value.eq_ignore_ascii_case("close"));
        let keep_alive = keep_alive && !client_close;
        if !valid_loopback_host(request.headers.get("host"), stream.local_addr().ok()) {
            let _ = write_error(stream, 403, "gateway_host_forbidden", keep_alive);
            if !keep_alive {
                return;
            }
            continue;
        }
        if request.method == "GET" && request.path == "/health" {
            let _ = write_response(
                stream,
                &GatewayResponse {
                    status: 200,
                    content_type: "application/json",
                    body: br#"{"ok":true,"service":"licoup-llm-gateway","protocols":["openai-responses","openai-chat-completions","anthropic-messages","live-provider-models"],"clientAuth":"bearer-or-x-api-key"}"#.to_vec(),
                },
                keep_alive,
            );
            if !keep_alive {
                return;
            }
            continue;
        }
        if request.method == "GET"
            && matches!(request.path.trim_end_matches('/'), "/v1/models" | "/models")
        {
            if !request_is_authorized(&request.headers, client_token) {
                let _ = write_error(stream, 401, "gateway_client_unauthorized", keep_alive);
            } else {
                match list_models(gateway, credentials) {
                    Ok(response) => {
                        let _ = write_response(stream, &response, keep_alive);
                    }
                    Err(error) => {
                        let (status, code) = transport_error(error);
                        let _ = write_error(stream, status, code, keep_alive);
                    }
                }
            }
            if !keep_alive {
                return;
            }
            continue;
        }
        if request.method != "POST" {
            let _ = write_error(stream, 405, "method_not_allowed", keep_alive);
            if !keep_alive {
                return;
            }
            continue;
        }
        if !request_is_authorized(&request.headers, client_token) {
            let _ = write_error(stream, 401, "gateway_client_unauthorized", keep_alive);
            if !keep_alive {
                return;
            }
            continue;
        }
        let user_agent = request.headers.get("user-agent").map(String::as_str);
        let anthropic_beta = request.headers.get("anthropic-beta").map(String::as_str);
        let mut sink = HttpGatewayStreamSink {
            stream,
            started: false,
        };
        let result = exchange_to_sink(
            gateway,
            &request.path,
            &request.body,
            user_agent,
            anthropic_beta,
            credentials,
            &mut sink,
        );
        if !matches!(&result, Err(GatewayTransportError::Gateway(_))) {
            usage.record(&request.path, user_agent, &request.body);
        }
        match result {
            Ok(GatewayExchange::Buffered(response)) => {
                let _ = write_response(sink.stream, &response, keep_alive);
            }
            Ok(GatewayExchange::Streamed) => {
                return;
            }
            Err(error) => {
                if !sink.started {
                    let (status, code) = transport_error(error);
                    let _ = write_error(sink.stream, status, code, keep_alive);
                } else {
                    return;
                }
            }
        }
        if !keep_alive {
            return;
        }
    }
}

struct HttpGatewayStreamSink<'a> {
    stream: &'a mut TcpStream,
    started: bool,
}

impl GatewayStreamSink for HttpGatewayStreamSink<'_> {
    fn begin(&mut self, status: u16, content_type: &str) -> std::io::Result<()> {
        write!(
            self.stream,
            "HTTP/1.1 {} {}\r\ncontent-type: {}\r\ncache-control: no-store\r\nconnection: close\r\n\r\n",
            status,
            reason_phrase(status),
            content_type
        )?;
        self.stream.flush()?;
        self.started = true;
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.stream.write_all(bytes)?;
        self.stream.flush()
    }
}

fn valid_loopback_host(host: Option<&String>, local: Option<SocketAddr>) -> bool {
    let (Some(host), Some(local)) = (host, local) else {
        return false;
    };
    let expected = format!("127.0.0.1:{}", local.port());
    host == "127.0.0.1" || host == &expected
}

fn request_is_authorized(headers: &BTreeMap<String, String>, client_token: &SecretBytes) -> bool {
    let authorization = headers
        .get("authorization")
        .and_then(|value| value.strip_prefix("Bearer "));
    let api_key = headers.get("x-api-key").map(String::as_str);
    match (authorization, api_key) {
        (Some(_), Some(_)) | (None, None) => false,
        (Some(token), None) | (None, Some(token)) => {
            crate::control::client_auth::token_matches(client_token, token)
        }
    }
}

fn read_request(stream: &mut TcpStream, carry: &mut Vec<u8>) -> Result<Option<HttpRequest>, u16> {
    let header_end = loop {
        if let Some(position) = find_header_end(carry) {
            if position.saturating_add(4) > MAX_HEADER_BYTES {
                return Err(431);
            }
            break position;
        }
        if carry.len() >= MAX_HEADER_BYTES {
            return Err(431);
        }
        let mut chunk = [0u8; 4096];
        let read = stream.read(&mut chunk).map_err(|_| 400u16)?;
        if read == 0 {
            if carry.is_empty() {
                return Ok(None);
            }
            return Err(400);
        }
        carry.extend_from_slice(&chunk[..read]);
    };
    let head = std::str::from_utf8(&carry[..header_end]).map_err(|_| 400u16)?;
    let (method, path, headers) = parse_head(head)?;
    if headers.contains_key("transfer-encoding") {
        return Err(400);
    }
    let content_length = headers
        .get("content-length")
        .map(|value| value.parse::<usize>().map_err(|_| 400u16))
        .transpose()?
        .unwrap_or(0);
    if content_length > MAX_GATEWAY_BODY_BYTES {
        return Err(413);
    }
    let body_start = header_end + 4;
    while carry.len().saturating_sub(body_start) < content_length {
        let remaining = content_length - carry.len().saturating_sub(body_start);
        let mut chunk = vec![0u8; remaining];
        let read = stream.read(&mut chunk).map_err(|_| 400u16)?;
        if read == 0 {
            return Err(400);
        }
        carry.extend_from_slice(&chunk[..read]);
    }
    let body = carry[body_start..body_start + content_length].to_vec();
    let consumed = body_start + content_length;
    carry.drain(..consumed);
    Ok(Some(HttpRequest {
        method,
        path,
        headers,
        body,
    }))
}

fn parse_head(head: &str) -> Result<(String, String, BTreeMap<String, String>), u16> {
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().ok_or(400u16)?.split_whitespace();
    let method = request_line.next().ok_or(400u16)?.to_owned();
    let path = request_line.next().ok_or(400u16)?.to_owned();
    let version = request_line.next().ok_or(400u16)?;
    if request_line.next().is_some()
        || version != "HTTP/1.1"
        || !path.starts_with('/')
        || path.contains('?')
        || path.contains('#')
    {
        return Err(400);
    }
    let mut headers = BTreeMap::new();
    for (count, line) in lines.enumerate() {
        if count >= MAX_HEADERS || line.starts_with(' ') || line.starts_with('\t') {
            return Err(431);
        }
        let (name, value) = line.split_once(':').ok_or(400u16)?;
        let name = name.to_ascii_lowercase();
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || value.bytes().any(|byte| byte < 0x20 && byte != b'\t')
            || headers.insert(name, value.trim().to_owned()).is_some()
        {
            return Err(400);
        }
    }
    Ok((method, path, headers))
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn write_response(
    stream: &mut TcpStream,
    response: &GatewayResponse,
    keep_alive: bool,
) -> std::io::Result<()> {
    let reason = reason_phrase(response.status);
    write!(
        stream,
        "HTTP/1.1 {} {}\r\ncontent-type: {}\r\ncontent-length: {}\r\ncache-control: no-store\r\nconnection: {}\r\n\r\n",
        response.status,
        reason,
        response.content_type,
        response.body.len(),
        if keep_alive { "keep-alive" } else { "close" }
    )?;
    stream.write_all(&response.body)
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500..=599 => "Upstream Failure",
        _ => "Response",
    }
}

fn write_error(
    stream: &mut TcpStream,
    status: u16,
    code: &str,
    keep_alive: bool,
) -> std::io::Result<()> {
    let body = serde_json::to_vec(&serde_json::json!({
        "error":{"message":code,"type":"gateway_error","code":code,"param":null}
    }))
    .unwrap_or_default();
    write_response(
        stream,
        &GatewayResponse {
            status,
            content_type: "application/json",
            body,
        },
        keep_alive,
    )
}

fn transport_error(error: GatewayTransportError) -> (u16, &'static str) {
    use crate::model::llm_gateway::GatewayError;
    match error {
        GatewayTransportError::Gateway(GatewayError::UnsupportedPath) => {
            (404, "gateway_path_not_found")
        }
        GatewayTransportError::Gateway(GatewayError::RouteNotFound) => {
            (404, "gateway_route_not_found")
        }
        GatewayTransportError::Gateway(GatewayError::RequestTooLarge)
        | GatewayTransportError::ResponseTooLarge => (413, "gateway_payload_too_large"),
        GatewayTransportError::Gateway(_) => (400, "gateway_request_invalid"),
        GatewayTransportError::CredentialUnavailable => (503, "gateway_credential_unavailable"),
        GatewayTransportError::Busy => (503, "gateway_busy"),
        GatewayTransportError::TransportFailed => (502, "gateway_upstream_unavailable"),
    }
}

pub fn bind_address(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::llm_gateway::{
        ClientProtocol, CredentialStyle, GatewayConfig, GatewayProvider, ModelRoute,
        UpstreamProtocol,
    };
    use anyhow::Result;
    use licoup_vault::api_key::{
        GatewayCredential, GatewayCredentialEpochSource, GatewayCredentialLease,
        GatewayCredentialLeaseDays, LlmApiKeyProvider,
    };
    use std::sync::Arc;
    use std::time::Duration;

    struct StaticEpochSource(String);

    impl GatewayCredentialEpochSource for StaticEpochSource {
        fn active_epoch(&self) -> Result<Option<String>> {
            Ok(Some(self.0.clone()))
        }
    }

    fn fixture_gateway(address: SocketAddr) -> CompiledGateway {
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

    fn fixture_slot(secret: &str) -> GatewayCredentialSlot {
        let epoch = uuid::Uuid::new_v4().to_string();
        let credentials = BTreeMap::from([(
            LlmApiKeyProvider::Kimi,
            vec![
                GatewayCredential::new(
                    uuid::Uuid::new_v4().to_string(),
                    SecretBytes::try_from_string(secret.to_string()).unwrap(),
                    None,
                )
                .unwrap(),
            ],
        )]);
        GatewayCredentialSlot::new(
            GatewayCredentialLease::new(
                credentials,
                GatewayCredentialLeaseDays::Seven,
                epoch.clone(),
                Arc::new(StaticEpochSource(epoch)),
            )
            .unwrap(),
        )
    }

    fn client_token() -> SecretBytes {
        SecretBytes::try_from_string("a".repeat(43)).unwrap()
    }

    const CHAT_RESPONSE: &str = r#"{"id":"chat-1","object":"chat.completion","created":1,"model":"upstream","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}"#;

    fn read_upstream_request(stream: &mut TcpStream) -> Option<Vec<u8>> {
        let mut received = Vec::new();
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            let read = match stream.read(&mut chunk) {
                Ok(0) | Err(_) => return None,
                Ok(read) => read,
            };
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
            let read = match stream.read(&mut chunk) {
                Ok(0) | Err(_) => return None,
                Ok(read) => read,
            };
            received.extend_from_slice(&chunk[..read]);
        }
        received.truncate(body_start + content_length);
        Some(received)
    }

    fn write_upstream_response(stream: &mut TcpStream, body: &str) {
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: keep-alive\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
        stream.flush().unwrap();
    }

    fn read_http_response(stream: &mut TcpStream) -> (u16, String, Vec<u8>) {
        let mut received = Vec::new();
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            let read = stream.read(&mut chunk).unwrap();
            assert!(read > 0, "gateway closed before the response completed");
            received.extend_from_slice(&chunk[..read]);
            if let Some(position) = received.windows(4).position(|part| part == b"\r\n\r\n") {
                break position;
            }
        };
        let head = String::from_utf8_lossy(&received[..header_end]).to_string();
        let mut lines = head.split("\r\n");
        let status = lines
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse::<u16>()
            .unwrap();
        let content_length = lines
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
                "gateway closed before the response body completed"
            );
            received.extend_from_slice(&chunk[..read]);
        }
        let body = received[body_start..body_start + content_length].to_vec();
        (status, head, body)
    }

    #[test]
    fn parser_accepts_bounded_http_and_rejects_smuggling_shapes() {
        let (_, path, headers) =
            parse_head("POST /v1/responses HTTP/1.1\r\nhost: 127.0.0.1\r\ncontent-length: 2")
                .unwrap();
        assert_eq!(path, "/v1/responses");
        assert_eq!(headers["content-length"], "2");
        assert!(parse_head("POST /v1/responses?x=1 HTTP/1.1\r\nhost: x").is_err());
        assert!(parse_head("POST /v1/responses HTTP/1.1\r\nx: 1\r\nx: 2").is_err());
        assert!(parse_head("POST /v1/responses HTTP/1.1\r\n folded: x").is_err());
    }

    #[test]
    fn public_bind_address_is_always_loopback() {
        assert!(bind_address(0).ip().is_loopback());
    }

    #[test]
    fn post_authentication_requires_one_exact_token() {
        let token = SecretBytes::try_from_string("a".repeat(43)).unwrap();
        let mut headers = BTreeMap::new();
        assert!(!request_is_authorized(&headers, &token));
        headers.insert("authorization".into(), format!("Bearer {}", "a".repeat(43)));
        assert!(request_is_authorized(&headers, &token));
        headers.insert("x-api-key".into(), "a".repeat(43));
        assert!(!request_is_authorized(&headers, &token));
        headers.remove("authorization");
        assert!(request_is_authorized(&headers, &token));
        headers.insert("x-api-key".into(), "wrong".into());
        assert!(!request_is_authorized(&headers, &token));
    }

    #[test]
    fn host_is_bound_to_the_listener_port() {
        let local = Some(SocketAddr::from(([127, 0, 0, 1], 15_722)));
        assert!(valid_loopback_host(Some(&"127.0.0.1".into()), local));
        assert!(valid_loopback_host(Some(&"127.0.0.1:15722".into()), local));
        assert!(!valid_loopback_host(Some(&"example.test".into()), local));
        assert!(!valid_loopback_host(Some(&"127.0.0.1:15723".into()), local));
    }

    #[test]
    fn read_request_consumes_pipelined_bytes_from_carry_without_extra_reads() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = stream.set_read_timeout(Some(Duration::from_millis(400)));
            let mut carry = Vec::new();
            let first = read_request(&mut stream, &mut carry).unwrap().unwrap();
            assert_eq!(first.path, "/v1/responses");
            assert_eq!(first.body, br#"{"model":"requested"}"#);
            let second = read_request(&mut stream, &mut carry).unwrap().unwrap();
            assert_eq!(second.path, "/v1/chat/completions");
            assert_eq!(second.body, br#"{"model":"requested"}"#);
            assert!(carry.is_empty());
        });
        let mut client = TcpStream::connect(address).unwrap();
        let body = br#"{"model":"requested"}"#;
        let pipelined = format!(
            "POST /v1/responses HTTP/1.1\r\nhost: {address}\r\ncontent-length: {}\r\n\r\n{}POST /v1/chat/completions HTTP/1.1\r\nhost: {address}\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            String::from_utf8_lossy(body),
            body.len(),
            String::from_utf8_lossy(body)
        );
        client.write_all(pipelined.as_bytes()).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn read_request_accepts_a_bounded_header_when_carry_already_contains_a_large_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let client = TcpStream::connect(address).unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        drop(client);

        let body = vec![b'x'; MAX_HEADER_BYTES];
        let mut carry = format!(
            "POST /v1/responses HTTP/1.1\r\nhost: {address}\r\ncontent-length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        carry.extend_from_slice(&body);

        let request = read_request(&mut stream, &mut carry).unwrap().unwrap();
        assert_eq!(request.path, "/v1/responses");
        assert_eq!(request.body.len(), body.len());
        assert!(carry.is_empty());
    }

    #[test]
    fn model_catalog_requires_local_auth_and_is_empty_without_provider_credentials() {
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        let gateway = Arc::new(fixture_gateway(upstream.local_addr().unwrap()));
        let credentials = Arc::new(GatewayCredentialSlot::disconnected());
        let token = Arc::new(client_token());
        let usage_root = std::env::temp_dir().join(format!(
            "licoup-gateway-model-server-usage-{}",
            uuid::Uuid::new_v4()
        ));
        licoup_foundation::platform::file_security::ensure_private_dir(&usage_root).unwrap();
        let usage = Arc::new(GatewayUsageRecorder::open(usage_root.join("usage.json")).unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server_address = listener.local_addr().unwrap();
        let server_thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handle_connection(&mut stream, &gateway, &credentials, &token, &usage);
        });
        let mut client = TcpStream::connect(server_address).unwrap();
        client
            .write_all(
                format!(
                    "GET /v1/models HTTP/1.1\r\nhost: {server_address}\r\nconnection: keep-alive\r\n\r\n"
                )
                .as_bytes(),
            )
            .unwrap();
        let (status, _, _) = read_http_response(&mut client);
        assert_eq!(status, 401);
        client
            .write_all(
                format!(
                    "GET /v1/models HTTP/1.1\r\nhost: {server_address}\r\nauthorization: Bearer {}\r\nconnection: close\r\n\r\n",
                    "a".repeat(43)
                )
                .as_bytes(),
            )
            .unwrap();
        let (status, _, body) = read_http_response(&mut client);
        assert_eq!(status, 200);
        let document: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(document["data"], serde_json::json!([]));
        drop(client);
        server_thread.join().unwrap();
        let _ = std::fs::remove_dir_all(usage_root);
    }

    #[test]
    fn one_socket_serves_sequential_requests_with_keep_alive_and_close() {
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        let upstream_address = upstream.local_addr().unwrap();
        let upstream_thread = thread::spawn(move || {
            let (mut stream, _) = upstream.accept().unwrap();
            for _ in 0..2 {
                assert!(read_upstream_request(&mut stream).is_some());
                write_upstream_response(&mut stream, CHAT_RESPONSE);
            }
        });
        let gateway = Arc::new(fixture_gateway(upstream_address));
        let credentials = Arc::new(fixture_slot("secret-key"));
        let token = Arc::new(client_token());
        let usage_root = std::env::temp_dir().join(format!(
            "licoup-gateway-server-usage-{}",
            uuid::Uuid::new_v4()
        ));
        licoup_foundation::platform::file_security::ensure_private_dir(&usage_root).unwrap();
        let usage_path = usage_root.join("usage.json");
        let usage = Arc::new(GatewayUsageRecorder::open(usage_path).unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server_address = listener.local_addr().unwrap();
        let server_thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handle_connection(&mut stream, &gateway, &credentials, &token, &usage);
        });
        let mut client = TcpStream::connect(server_address).unwrap();
        let _ = client.set_read_timeout(Some(Duration::from_secs(15)));
        let body = br#"{"model":"requested","stream":false}"#;
        let body_text = String::from_utf8_lossy(body);
        let token_text = "a".repeat(43);
        let post = |path: &str, close: bool| {
            format!(
                "POST {} HTTP/1.1\r\nhost: {}\r\nauthorization: Bearer {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: {}\r\n\r\n{}",
                path,
                server_address,
                token_text,
                body.len(),
                if close { "close" } else { "keep-alive" },
                body_text
            )
        };
        client
            .write_all(post("/v1/chat/completions", false).as_bytes())
            .unwrap();
        let (status, head, response_body) = read_http_response(&mut client);
        assert_eq!(status, 200);
        assert!(
            head.to_ascii_lowercase().contains("connection: keep-alive"),
            "first response must keep the socket alive: {head}"
        );
        assert!(String::from_utf8_lossy(&response_body).contains("ok"));
        client
            .write_all(post("/v1/chat/completions", true).as_bytes())
            .unwrap();
        let (status, head, _) = read_http_response(&mut client);
        assert_eq!(status, 200);
        assert!(
            head.to_ascii_lowercase().contains("connection: close"),
            "client-requested close must terminate the socket: {head}"
        );
        drop(client);
        server_thread.join().unwrap();
        upstream_thread.join().unwrap();
    }

    #[test]
    fn invalid_host_gets_403_and_the_socket_keeps_serving() {
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        let upstream_address = upstream.local_addr().unwrap();
        let upstream_thread = thread::spawn(move || {
            let (mut stream, _) = upstream.accept().unwrap();
            assert!(read_upstream_request(&mut stream).is_some());
            write_upstream_response(&mut stream, CHAT_RESPONSE);
        });
        let gateway = Arc::new(fixture_gateway(upstream_address));
        let credentials = Arc::new(fixture_slot("secret-key"));
        let token = Arc::new(client_token());
        let usage_root = std::env::temp_dir().join(format!(
            "licoup-gateway-server-usage-{}",
            uuid::Uuid::new_v4()
        ));
        licoup_foundation::platform::file_security::ensure_private_dir(&usage_root).unwrap();
        let usage_path = usage_root.join("usage.json");
        let usage = Arc::new(GatewayUsageRecorder::open(usage_path).unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server_address = listener.local_addr().unwrap();
        let server_thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handle_connection(&mut stream, &gateway, &credentials, &token, &usage);
        });
        let mut client = TcpStream::connect(server_address).unwrap();
        let _ = client.set_read_timeout(Some(Duration::from_secs(15)));
        let body = br#"{"model":"requested","stream":false}"#;
        let body_text = String::from_utf8_lossy(body);
        let token_text = "a".repeat(43);
        let request = |host: &str| {
            format!(
                "POST /v1/chat/completions HTTP/1.1\r\nhost: {host}\r\nauthorization: Bearer {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                token_text,
                body.len(),
                body_text
            )
        };
        client
            .write_all(request("example.test").as_bytes())
            .unwrap();
        let (status, head, _) = read_http_response(&mut client);
        assert_eq!(status, 403);
        assert!(
            head.to_ascii_lowercase().contains("connection: keep-alive"),
            "rejected host must not close the socket: {head}"
        );
        client
            .write_all(request(&format!("127.0.0.1:{}", server_address.port())).as_bytes())
            .unwrap();
        let (status, _, response_body) = read_http_response(&mut client);
        assert_eq!(status, 200);
        assert!(String::from_utf8_lossy(&response_body).contains("ok"));
        drop(client);
        server_thread.join().unwrap();
        upstream_thread.join().unwrap();
    }
}
