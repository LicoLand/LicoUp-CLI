//! Bounded HTTP fetches for provider quota endpoints.
//!
//! Credentials travel only in request headers built in memory; responses are
//! bounded in size and only the parsed JSON body leaves these helpers. Hosted
//! URLs are restricted to HTTPS per the platform URL security policy; the
//! loopback lane is restricted to 127.0.0.1 / localhost / ::1 so the
//! self-signed-accepting TLS agent it carries can never leave this machine.

use super::contract::QuotaFetchError;
use serde_json::Value;
use std::io::Read;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use ureq::rustls;

pub(super) const HOSTED_FETCH_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const LOOPBACK_FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE_BYTES: u64 = 512 * 1024;

fn hosted_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(2))
            .build()
    })
}

fn loopback_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            // Loopback quota traffic must stay local and deterministic; never
            // inherit a process proxy for it.
            .try_proxy_from_env(false)
            // The local language server serves a self-signed certificate
            // (CN=localhost), so the loopback lane carries its own rustls
            // config that accepts self-signed chains. Hosted agents keep the
            // default root store; the loopback-only URL policy guarantees this
            // insecure verifier can never reach a public host.
            .tls_config(loopback_tls_config())
            .timeout_connect(Duration::from_secs(2))
            .build()
    })
}

/// rustls `ClientConfig` for the loopback lane: accept any server certificate
/// (the local Antigravity runtime signs its own) but still verify the TLS
/// handshake signatures against the certificate's public key with the
/// provider's standard algorithms.
///
/// No new dependency is needed: ureq re-exports the exact rustls version it
/// was built against as `ureq::rustls`, and the workspace already builds that
/// rustls with the `ring` provider via ureq's default `tls` feature.
fn loopback_tls_config() -> Arc<rustls::ClientConfig> {
    let provider = rustls::crypto::CryptoProvider::get_default()
        .map(|default| (**default).clone())
        .unwrap_or_else(rustls::crypto::ring::default_provider);
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(provider.clone()))
        .with_safe_default_protocol_versions()
        .expect("ring provider supports the default TLS protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptSelfSignedServerVerifier {
            algorithms: provider.signature_verification_algorithms,
        }))
        .with_no_client_auth();
    Arc::new(config)
}

/// Verifier that accepts any presented certificate chain — the local server
/// serves a self-signed certificate — while still validating the TLS
/// handshake signatures, so only a peer holding the certificate's private key
/// can complete the connection.
#[derive(Debug)]
struct AcceptSelfSignedServerVerifier {
    algorithms: rustls::crypto::WebPkiSupportedAlgorithms,
}

impl rustls::client::danger::ServerCertVerifier for AcceptSelfSignedServerVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

fn validate_url(url: &str, code: &'static str) -> Result<(), QuotaFetchError> {
    if licoup_foundation::platform::url_security::is_https_or_loopback_http_url(url) {
        Ok(())
    } else {
        Err(QuotaFetchError::new(code))
    }
}

pub(super) fn get_json_with_bearer(
    url: &str,
    bearer: &str,
    timeout: Duration,
) -> Result<Value, QuotaFetchError> {
    get_json_with_header(url, "Authorization", &format!("Bearer {bearer}"), timeout)
}

pub(super) fn get_json_with_cookie(
    url: &str,
    cookie: &str,
    timeout: Duration,
) -> Result<Value, QuotaFetchError> {
    get_json_with_header(url, "Cookie", cookie, timeout)
}

fn get_json_with_header(
    url: &str,
    header: &str,
    value: &str,
    timeout: Duration,
) -> Result<Value, QuotaFetchError> {
    get_json_with_headers(url, &[(header, value)], timeout)
}

/// Bounded hosted GET carrying several in-memory headers (for example a
/// bearer token plus device identity headers).
pub(super) fn get_json_with_headers(
    url: &str,
    headers: &[(&str, &str)],
    timeout: Duration,
) -> Result<Value, QuotaFetchError> {
    validate_url(url, "quota_endpoint_url_rejected")?;
    let mut request = hosted_agent().get(url).timeout(timeout);
    for (name, value) in headers {
        request = request.set(name, value);
    }
    let response = request
        .call()
        .map_err(|_| QuotaFetchError::new("quota_endpoint_request_failed"))?;
    decode_bounded_json(response)
}

/// Loopback-only URL policy for the self-signed-accepting lane. Unlike the
/// hosted policy (any HTTPS), this rejects every non-loopback address —
/// public HTTPS included — so the insecure TLS agent can never leave the
/// machine even if a caller passes an arbitrary URL.
fn validate_loopback_url(url: &str, code: &'static str) -> Result<(), QuotaFetchError> {
    if is_loopback_only_url(url) {
        Ok(())
    } else {
        Err(QuotaFetchError::new(code))
    }
}

fn is_loopback_only_url(value: &str) -> bool {
    // Reuse the shared parser's strictness (no userinfo, fragments, spoofed
    // hosts, port 0), then require the host to be a loopback address.
    if !licoup_foundation::platform::url_security::is_https_or_loopback_http_url(value) {
        return false;
    }
    match url::Url::parse(value.trim()).ok() {
        Some(parsed) => match parsed.host() {
            Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(address)) => address.octets() == [127, 0, 0, 1],
            Some(url::Host::Ipv6(address)) => address.is_loopback(),
            None => false,
        },
        None => false,
    }
}

/// Build and send a loopback POST; the caller interprets the response. The
/// response body is never read here.
fn loopback_post(
    url: &str,
    headers: &[(&str, &str)],
    body: &[u8],
    timeout: Duration,
) -> Result<ureq::Response, QuotaFetchError> {
    validate_loopback_url(url, "quota_loopback_url_rejected")?;
    let mut request = loopback_agent()
        .post(url)
        .timeout(timeout)
        .set("Content-Type", "application/json");
    for (name, value) in headers {
        request = request.set(name, value);
    }
    request
        .send_bytes(body)
        .map_err(|_| QuotaFetchError::new("quota_loopback_request_failed"))
}

pub(super) fn post_json_loopback(
    url: &str,
    headers: &[(&str, &str)],
    body: &[u8],
    timeout: Duration,
) -> Result<Value, QuotaFetchError> {
    let response = loopback_post(url, headers, body, timeout)?;
    decode_bounded_json(response)
}

/// Loopback POST that only requires a 2xx status. Used to probe candidate
/// listen ports without depending on the probe endpoint's payload shape.
pub(super) fn post_loopback_status(
    url: &str,
    headers: &[(&str, &str)],
    body: &[u8],
    timeout: Duration,
) -> Result<(), QuotaFetchError> {
    let response = loopback_post(url, headers, body, timeout)?;
    if (200..300).contains(&response.status()) {
        Ok(())
    } else {
        Err(QuotaFetchError::new("quota_loopback_request_failed"))
    }
}

fn decode_bounded_json(response: ureq::Response) -> Result<Value, QuotaFetchError> {
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_RESPONSE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| QuotaFetchError::new("quota_response_read_failed"))?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(QuotaFetchError::new("quota_response_too_large"));
    }
    serde_json::from_slice(&bytes).map_err(|_| QuotaFetchError::new("quota_response_invalid_json"))
}
