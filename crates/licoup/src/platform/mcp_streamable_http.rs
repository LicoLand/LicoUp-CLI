//! Bounded MCP Streamable HTTP POST transport.
//!
//! Approval and JSON-RPC semantics stay in `domain::mcp_adapter`; this module
//! owns only endpoint validation, protocol headers, bounded concurrency and
//! bounded HTTP response collection.

use crate::core::mcp::{
    DEFAULT_MAX_MESSAGE_BYTES, McpTransferPacket, OUTBOUND_TRANSFER_PROTOCOL_REVISION,
};
use anyhow::{Result, anyhow, ensure};
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use url::{Host, Url};

const MAX_HTTP_HEADERS: usize = 128;
const MAX_HTTP_HEADER_BYTES: usize = 64 * 1024;
const MAX_HTTP_IN_FLIGHT: usize = 8;
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

static HTTP_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

pub(crate) struct McpStreamableHttpResponse {
    pub(crate) status: u16,
    pub(crate) content_type: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) body: Vec<u8>,
}

pub(crate) fn exchange(
    packet: &McpTransferPacket,
    session_id: Option<&str>,
) -> Result<McpStreamableHttpResponse> {
    let endpoint = validate_endpoint(packet.destination())?;
    validate_session_id(session_id)?;
    let _permit = HttpPermit::acquire()?;
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(3))
        .timeout_read(HTTP_TIMEOUT)
        .timeout_write(HTTP_TIMEOUT)
        .build();
    let mut request = agent
        .post(endpoint.as_str())
        .set("accept", "application/json, text/event-stream")
        .set("content-type", "application/json")
        .set("mcp-protocol-version", OUTBOUND_TRANSFER_PROTOCOL_REVISION);
    if let Some(session_id) = session_id {
        request = request.set("mcp-session-id", session_id);
    }
    let response = match request.send_bytes(packet.body()) {
        Ok(response) => response,
        Err(ureq::Error::Status(_, _)) => return Err(anyhow!("mcp_http_status_error")),
        Err(ureq::Error::Transport(_)) => return Err(anyhow!("mcp_http_transport_failed")),
    };
    validate_response_headers(&response)?;
    let status = response.status();
    let content_type = response
        .header("content-type")
        .map(|value| value.to_ascii_lowercase());
    let session_id = response.header("mcp-session-id").map(str::to_owned);
    validate_session_id(session_id.as_deref())?;
    let mut body = Vec::new();
    response
        .into_reader()
        .take((DEFAULT_MAX_MESSAGE_BYTES as u64).saturating_add(1))
        .read_to_end(&mut body)
        .map_err(|_| anyhow!("mcp_http_response_read_failed"))?;
    ensure!(
        body.len() <= DEFAULT_MAX_MESSAGE_BYTES,
        "mcp_message_too_large"
    );
    Ok(McpStreamableHttpResponse {
        status,
        content_type,
        session_id,
        body,
    })
}

fn validate_endpoint(raw: &str) -> Result<Url> {
    let endpoint = Url::parse(raw).map_err(|_| anyhow!("mcp_http_endpoint_invalid"))?;
    ensure!(
        !endpoint.cannot_be_a_base()
            && endpoint.username().is_empty()
            && endpoint.password().is_none()
            && endpoint.fragment().is_none(),
        "mcp_http_endpoint_invalid"
    );
    let loopback = match endpoint
        .host()
        .ok_or_else(|| anyhow!("mcp_http_endpoint_invalid"))?
    {
        Host::Domain(host) => host.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => address.is_loopback(),
    };
    ensure!(
        endpoint.scheme() == "https" || endpoint.scheme() == "http" && loopback,
        "mcp_http_endpoint_requires_https"
    );
    Ok(endpoint)
}

fn validate_session_id(session_id: Option<&str>) -> Result<()> {
    let Some(session_id) = session_id else {
        return Ok(());
    };
    ensure!(
        !session_id.is_empty()
            && session_id.len() <= 1024
            && session_id.bytes().all(|byte| (0x21..=0x7e).contains(&byte)),
        "mcp_session_id_invalid"
    );
    Ok(())
}

fn validate_response_headers(response: &ureq::Response) -> Result<()> {
    let names = response.headers_names();
    ensure!(
        names.len() <= MAX_HTTP_HEADERS,
        "mcp_http_headers_too_large"
    );
    let total = names.iter().try_fold(0usize, |total, name| {
        response
            .all(name)
            .iter()
            .try_fold(total.saturating_add(name.len()), |subtotal, value| {
                let next = subtotal.saturating_add(value.len());
                (next <= MAX_HTTP_HEADER_BYTES).then_some(next)
            })
    });
    ensure!(total.is_some(), "mcp_http_headers_too_large");
    Ok(())
}

struct HttpPermit;

impl HttpPermit {
    fn acquire() -> Result<Self> {
        let acquired = HTTP_IN_FLIGHT
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < MAX_HTTP_IN_FLIGHT).then_some(active + 1)
            })
            .is_ok();
        ensure!(acquired, "mcp_http_transport_busy");
        Ok(Self)
    }
}

impl Drop for HttpPermit {
    fn drop(&mut self) {
        HTTP_IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_policy_allows_https_and_loopback_http_only() {
        assert!(validate_endpoint("https://example.invalid/mcp").is_ok());
        assert!(validate_endpoint("http://127.0.0.1:3000/mcp").is_ok());
        assert!(validate_endpoint("http://[::1]:3000/mcp").is_ok());
        assert!(validate_endpoint("http://example.invalid/mcp").is_err());
        assert!(validate_endpoint("https://name:secret@example.invalid/mcp").is_err());
    }
}
