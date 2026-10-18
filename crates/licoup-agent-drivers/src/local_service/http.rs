use licoup_foundation::platform::raw_execution::{RawExecutionDirection, RawExecutionObserver};
use serde_json::{Value, json};
use std::io::Read;
use std::sync::OnceLock;
use std::time::Duration;
use url::Url;

use super::bounds::{
    CONCURRENCY_WAIT, MAX_HTTP_HEADER_BYTES, MAX_HTTP_HEADER_COUNT, MAX_HTTP_IN_FLIGHT,
    MAX_HTTP_REQUEST_BODY_BYTES, MAX_HTTP_RESPONSE_BODY_BYTES,
};
use super::concurrency::{BoundedGate, LimitFailure};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpFailure {
    BodyTooLarge,
    Busy,
    HeadersTooLarge,
    InvalidJson,
    InvalidUrl,
    NotFound,
    Request,
    Serialize,
    Status(u16),
    Unavailable,
}

fn request_gate() -> &'static BoundedGate {
    static GATE: OnceLock<BoundedGate> = OnceLock::new();
    GATE.get_or_init(|| BoundedGate::new(MAX_HTTP_IN_FLIGHT))
}

pub fn get_json(url: &str, timeout: Duration) -> Result<Value, HttpFailure> {
    get_json_recorded(url, timeout, None)
}

pub fn get_json_observed(url: &str, timeout: Duration, source: &str) -> Result<Value, HttpFailure> {
    get_json_recorded(url, timeout, Some(source))
}

fn get_json_recorded(
    url: &str,
    timeout: Duration,
    source: Option<&str>,
) -> Result<Value, HttpFailure> {
    let url = validate_url(url)?;
    let _permit = request_gate()
        .acquire(CONCURRENCY_WAIT)
        .map_err(map_limit_failure)?;
    let request = control_agent().get(url.as_str()).timeout(timeout);
    record_request_metadata(source, &request);
    let response = map_response(request.call(), source)?;
    decode_json(response, source)
}

pub fn post_json_observed(
    url: &str,
    body: &Value,
    timeout: Option<Duration>,
    source: &str,
) -> Result<Value, HttpFailure> {
    post_json_recorded(url, body, timeout, Some(source))
}

fn post_json_recorded(
    url: &str,
    body: &Value,
    timeout: Option<Duration>,
    source: Option<&str>,
) -> Result<Value, HttpFailure> {
    let url = validate_url(url)?;
    let bytes = serde_json::to_vec(body).map_err(|_| HttpFailure::Serialize)?;
    if bytes.len() > MAX_HTTP_REQUEST_BODY_BYTES {
        return Err(HttpFailure::BodyTooLarge);
    }
    let _permit = request_gate()
        .acquire(CONCURRENCY_WAIT)
        .map_err(map_limit_failure)?;
    let mut request = control_agent().post(url.as_str());
    if let Some(timeout) = timeout {
        request = request.timeout(timeout);
    }
    let request = request.set("Content-Type", "application/json");
    record_request_metadata(source, &request);
    record_body(source, RawExecutionDirection::Sent, &bytes);
    let response = map_response(request.send_bytes(&bytes), source)?;
    decode_json(response, source)
}

pub fn probe_status(url: &str, timeout: Duration) -> Result<u16, HttpFailure> {
    let url = validate_url(url)?;
    let _permit = request_gate()
        .acquire(CONCURRENCY_WAIT)
        .map_err(map_limit_failure)?;
    match control_agent().get(url.as_str()).timeout(timeout).call() {
        Ok(response) => {
            validate_headers(&response)?;
            Ok(response.status())
        }
        Err(ureq::Error::Status(status, response)) => {
            validate_headers(&response)?;
            Ok(status)
        }
        Err(_) => Err(HttpFailure::Request),
    }
}

pub fn validate_url(raw: &str) -> Result<Url, HttpFailure> {
    let url = Url::parse(raw).map_err(|_| HttpFailure::InvalidUrl)?;
    if licoup_foundation::platform::url_security::is_https_or_loopback_http_url(raw) {
        Ok(url)
    } else {
        Err(HttpFailure::InvalidUrl)
    }
}

pub fn validate_headers(response: &ureq::Response) -> Result<(), HttpFailure> {
    let names = response.headers_names();
    if names.len() > MAX_HTTP_HEADER_COUNT {
        return Err(HttpFailure::HeadersTooLarge);
    }
    let total = names.iter().try_fold(0usize, |total, name| {
        response
            .all(name)
            .iter()
            .try_fold(total.saturating_add(name.len()), |subtotal, value| {
                let next = subtotal.saturating_add(value.len());
                (next <= MAX_HTTP_HEADER_BYTES).then_some(next)
            })
    });
    if total.is_none() {
        return Err(HttpFailure::HeadersTooLarge);
    }
    Ok(())
}

/// One shared client owned by the local serving control plane. Health probes,
/// attach probes, and turn-control operations all reuse this client so warm
/// traffic reuses its connection pool; per-request deadlines are applied on
/// each request instead of rebuilding an agent.
fn control_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            // Native serve adapters are local control planes. Never inherit a
            // process proxy here: loopback abort/prompt traffic must stay local,
            // deterministic, and independent from proxy environment state.
            .try_proxy_from_env(false)
            .timeout_connect(Duration::from_secs(2))
            .build()
    })
}

fn map_response(
    response: Result<ureq::Response, ureq::Error>,
    source: Option<&str>,
) -> Result<ureq::Response, HttpFailure> {
    match response {
        Ok(response) => {
            record_response_metadata(source, &response);
            validate_headers(&response)?;
            Ok(response)
        }
        Err(ureq::Error::Status(404, response)) => {
            record_response_metadata(source, &response);
            validate_headers(&response)?;
            if source.is_some() && RawExecutionObserver::current().is_some() {
                let _ = read_body(response, source);
            }
            Err(HttpFailure::NotFound)
        }
        Err(ureq::Error::Status(status, response)) => {
            record_response_metadata(source, &response);
            validate_headers(&response)?;
            if source.is_some() && RawExecutionObserver::current().is_some() {
                let _ = read_body(response, source);
            }
            Err(HttpFailure::Status(status))
        }
        Err(_) => Err(HttpFailure::Request),
    }
}

fn decode_json(response: ureq::Response, source: Option<&str>) -> Result<Value, HttpFailure> {
    let bytes = read_body(response, source)?;
    serde_json::from_slice(&bytes).map_err(|_| HttpFailure::InvalidJson)
}

fn read_body(response: ureq::Response, source: Option<&str>) -> Result<Vec<u8>, HttpFailure> {
    let mut bytes = Vec::new();
    let read_result = response
        .into_reader()
        .take((MAX_HTTP_RESPONSE_BODY_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| HttpFailure::Request);
    record_body(source, RawExecutionDirection::Received, &bytes);
    read_result?;
    if bytes.len() > MAX_HTTP_RESPONSE_BODY_BYTES {
        return Err(HttpFailure::BodyTooLarge);
    }
    Ok(bytes)
}

fn record_body(source: Option<&str>, direction: RawExecutionDirection, bytes: &[u8]) {
    if let Some(source) = source
        && let Some(observer) = RawExecutionObserver::current()
    {
        observer.record_bytes(source, direction, bytes);
    }
}

fn map_limit_failure(failure: LimitFailure) -> HttpFailure {
    match failure {
        LimitFailure::Busy => HttpFailure::Busy,
        LimitFailure::Unavailable => HttpFailure::Unavailable,
    }
}

// These are the values exposed by ureq, not reconstructed wire headers.
pub fn record_request_metadata(source: Option<&str>, request: &ureq::Request) {
    let (Some(source), Some(observer)) = (source, RawExecutionObserver::current()) else {
        return;
    };
    let mut names = request.header_names();
    names.sort();
    names.dedup();
    let headers: Vec<_> = names
        .iter()
        .map(|name| json!({"name": name, "values": request.all(name)}))
        .collect();
    observer.record(
        &format!("{}.request-metadata", source),
        RawExecutionDirection::Sent,
        &json!({
            "method": request.method(), "url": request.url(), "headers": headers
        })
        .to_string(),
    );
}

pub fn record_response_metadata(source: Option<&str>, response: &ureq::Response) {
    let (Some(source), Some(observer)) = (source, RawExecutionObserver::current()) else {
        return;
    };
    let mut names = response.headers_names();
    names.sort();
    names.dedup();
    let headers: Vec<_> = names
        .iter()
        .map(|name| json!({"name": name, "values": response.all(name)}))
        .collect();
    observer.record(
        &format!("{}.response-metadata", source),
        RawExecutionDirection::Received,
        &json!({
            "url": response.get_url(), "httpVersion": response.http_version(),
            "status": response.status(), "statusText": response.status_text(), "headers": headers
        })
        .to_string(),
    );
}
