use licoup_foundation::core::secret_bytes::SecretBytes;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

#[derive(Debug)]
pub enum TransportError {
    NonLoopback,
    Connect,
    Protocol(String),
    Io(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonLoopback => write!(f, "gateway_base_url_must_be_loopback"),
            Self::Connect => write!(f, "gateway_connect_failed"),
            Self::Protocol(m) | Self::Io(m) => write!(f, "{m}"),
        }
    }
}

pub trait LlmTransport: Send + Sync {
    fn complete(
        &self,
        model: &str,
        messages: &[Value],
        tools: &[Value],
    ) -> Result<Value, TransportError>;
}

pub struct GatewayChatTransport {
    host: String,
    port: u16,
    client_token: Option<SecretBytes>,
}

impl GatewayChatTransport {
    pub fn from_base_url(base_url: &str) -> Result<Self, TransportError> {
        let trimmed = base_url.trim().trim_end_matches('/');
        let without_scheme = trimmed
            .strip_prefix("http://")
            .ok_or(TransportError::NonLoopback)?;
        let (host, port_str) = without_scheme
            .split_once(':')
            .ok_or(TransportError::NonLoopback)?;
        if host != "127.0.0.1" && host != "localhost" {
            return Err(TransportError::NonLoopback);
        }
        let port: u16 = port_str
            .split('/')
            .next()
            .unwrap_or(port_str)
            .parse()
            .map_err(|_| TransportError::NonLoopback)?;
        Ok(Self {
            host: host.to_string(),
            port,
            client_token: None,
        })
    }

    /// Local client credential for the gateway's private loopback auth: the
    /// same `client-token` the runtime writes for `gateway client-token`
    /// helpers. Without it an auth-enforcing gateway answers 401.
    pub fn with_client_token(mut self, token: SecretBytes) -> Self {
        self.client_token = Some(token);
        self
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl LlmTransport for GatewayChatTransport {
    fn complete(
        &self,
        model: &str,
        messages: &[Value],
        tools: &[Value],
    ) -> Result<Value, TransportError> {
        let mut body = json!({
            "model": model,
            "messages": messages,
            "stream": false,
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
        }
        let body_bytes =
            serde_json::to_vec(&body).map_err(|e| TransportError::Protocol(e.to_string()))?;
        let addr = format!("{}:{}", self.host, self.port);
        let mut stream = TcpStream::connect_timeout(
            &addr.parse().map_err(|_| TransportError::Connect)?,
            Duration::from_secs(5),
        )
        .map_err(|_| TransportError::Connect)?;
        stream.set_read_timeout(Some(Duration::from_secs(180))).ok();
        stream.set_write_timeout(Some(Duration::from_secs(30))).ok();
        let authorization = self
            .client_token
            .as_ref()
            .and_then(|token| token.expose_utf8().ok())
            .map(|token| format!("Authorization: Bearer {token}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "POST /v1/chat/completions HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n{authorization}Content-Length: {}\r\nConnection: close\r\n\r\n",
            self.host,
            body_bytes.len()
        );
        stream
            .write_all(request.as_bytes())
            .map_err(|e| TransportError::Io(e.to_string()))?;
        stream
            .write_all(&body_bytes)
            .map_err(|e| TransportError::Io(e.to_string()))?;
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .map_err(|e| TransportError::Io(e.to_string()))?;
        let text = String::from_utf8_lossy(&response);
        let body_start = text
            .find("\r\n\r\n")
            .map(|i| i + 4)
            .ok_or_else(|| TransportError::Protocol("invalid_http_response".into()))?;
        let json_body = &text[body_start..];
        serde_json::from_str(json_body).map_err(|e| TransportError::Protocol(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_loopback() {
        assert!(GatewayChatTransport::from_base_url("http://example.com:15722").is_err());
        assert!(GatewayChatTransport::from_base_url("https://127.0.0.1:15722").is_err());
    }

    #[test]
    fn accepts_loopback() {
        let t = GatewayChatTransport::from_base_url("http://127.0.0.1:15722").unwrap();
        assert_eq!(t.port(), 15722);
    }

    #[test]
    fn client_token_lands_as_bearer_authorization() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 4096];
            let body_start = loop {
                let read = stream.read(&mut chunk).unwrap();
                assert!(read > 0);
                request.extend_from_slice(&chunk[..read]);
                if let Some(position) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    break position + 4;
                }
            };
            // Drain the request body too: closing with unread bytes resets
            // the connection before the response can be read.
            let content_length = String::from_utf8_lossy(&request[..body_start])
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            while request.len() < body_start + content_length {
                let read = stream.read(&mut chunk).unwrap();
                assert!(read > 0);
                request.extend_from_slice(&chunk[..read]);
            }
            let body = r#"{"choices":[{"message":{"content":"OK"}}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            request
        });
        let transport = GatewayChatTransport::from_base_url(&format!("http://127.0.0.1:{port}"))
            .unwrap()
            .with_client_token(SecretBytes::try_from_string("test-token".to_owned()).unwrap());
        let response = transport.complete("model", &[], &[]).unwrap();
        assert_eq!(response["choices"][0]["message"]["content"], "OK");
        let captured = server.join().unwrap();
        let request = String::from_utf8_lossy(&captured);
        assert!(
            request.contains("Authorization: Bearer test-token"),
            "{request}"
        );
    }
}
