//! The endpoint contract this Agent owns.
//!
//! Kilo Code's headless program runs a local HTTP service, and *how* it is
//! started, *where* it listens, *what* it exposes and *how* it reports failure
//! are facts about Kilo Code — not about LicoUp. They live here, in the package
//! that carries the Agent, as data. The client's serve engine reads them and
//! supplies the two things it owns: the environment it launches in and the
//! readiness reader that turns this Agent's documents into shared readiness.
//!
//! The reserved-port list is a declaration, not a policy the package enforces:
//! it names the ports this Agent must not take because the client, its
//! gateways and its peers already use them. A port outside the list and outside
//! the range the engine scans is refused by the engine, not silently accepted.
//!
//! Every failure code here is this Agent's own. The engine reports the code it
//! was given, so a reader can tell a missing Kilo executable from a missing
//! Codex one without parsing a message.

/// The ports this Agent's endpoint must not take: the client's own listeners,
/// its gateways and the peers a serve endpoint can run beside.
pub const RESERVED_PORTS: &[u16] = &[
    3000, 4096, 5173, 7228, 8080, 8443, 17328, 17329, 18765, 18789, 19001, 24173, 24174, 24175,
    24176, 24177, 24178, 24179, 24180, 24181, 24182, 24183, 24184, 24185, 24186, 24187, 24188,
    24189, 58627,
];

/// The environment variables an operator may point this Agent's executable at,
/// in the order they are consulted.
pub const EXECUTABLE_ENVIRONMENT: &[&str] = &["KILO_BIN", "KILO_PATH", "KILOCODE_PATH"];

/// The stable failure codes this Agent's endpoint reports.
///
/// They are a value rather than a `match` inside the engine so the package
/// names its own failures and the engine reports them unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServeErrorCodes {
    pub executable_missing: &'static str,
    pub port_exhausted: &'static str,
    pub start_failed: &'static str,
    pub health_failed: &'static str,
    pub attach_probe_failed: &'static str,
    pub not_found: &'static str,
    pub request_failed: &'static str,
    pub invalid_json: &'static str,
    pub invalid_state: &'static str,
    pub stop_failed: &'static str,
}

/// One Agent's serve endpoint contract.
///
/// The client adopts it field for field; nothing here is interpreted by the
/// package, because starting and supervising the service is the engine's work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServePolicy {
    /// The endpoint's durable identity: the state directory and pid record name.
    pub identity: &'static str,
    /// The port the endpoint prefers.
    pub default_port: u16,
    /// How far past [`Self::default_port`] the engine may scan.
    pub port_range_span: u16,
    /// The host the endpoint binds.
    pub default_host: &'static str,
    /// The path the health document is served at.
    pub health_path: &'static str,
    /// The path a session collection is served at.
    pub session_probe_path: &'static str,
    /// The path the endpoint's own configuration is served at.
    pub config_path: &'static str,
    /// The path the provider catalogue is served at.
    pub provider_path: &'static str,
    /// The state directory name the durable endpoint record lives under.
    pub state_dir: &'static str,
    /// The schema generation of that record.
    pub state_schema_version: &'static str,
    /// How long the engine waits for the endpoint to become healthy.
    pub default_health_timeout_ms: u64,
    /// Ports this Agent must not take.
    pub reserved_ports: &'static [u16],
    /// The environment variables that may name this Agent's executable.
    pub executable_environment: &'static [&'static str],
    /// The executable name the package looks for by default.
    pub default_executable: &'static str,
    /// The stable failure codes this Agent reports.
    pub errors: ServeErrorCodes,
}

/// The endpoint contract of the Kilo Code adapter package.
pub const SPEC: ServePolicy = ServePolicy {
    identity: "kilo_code_serve",
    default_port: 4097,
    port_range_span: 19,
    default_host: "127.0.0.1",
    health_path: "/global/health",
    session_probe_path: "/session",
    config_path: "/config",
    provider_path: "/provider",
    state_dir: "kilo-code-serve",
    state_schema_version: "v0.0.1:kilo-code-serve-2",
    default_health_timeout_ms: 45_000,
    reserved_ports: RESERVED_PORTS,
    executable_environment: EXECUTABLE_ENVIRONMENT,
    default_executable: "kilo",
    errors: ServeErrorCodes {
        executable_missing: "kilo_executable_missing",
        port_exhausted: "kilo_code_serve_port_exhausted",
        start_failed: "kilo_code_serve_start_failed",
        health_failed: "kilo_code_serve_health_failed",
        attach_probe_failed: "kilo_code_serve_attach_probe_failed",
        not_found: "kilo_code_serve_not_found",
        request_failed: "kilo_code_serve_request_failed",
        invalid_json: "kilo_code_serve_invalid_json",
        invalid_state: "kilo_code_serve_state_invalid",
        stop_failed: "kilo_code_serve_stop_failed",
    },
};

/// The endpoint's readiness document path, absolute against one attach URL.
pub fn endpoint_url(attach_url: &str, path: &str) -> String {
    format!("{}{}", attach_url.trim_end_matches('/'), path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_names_its_own_identity_ports_and_failures() {
        assert_eq!(SPEC.default_port, 4097);
        assert_eq!(SPEC.identity, "kilo_code_serve");
        assert_eq!(SPEC.default_executable, "kilo");
        assert_eq!(SPEC.errors.executable_missing, "kilo_executable_missing");
        assert_eq!(SPEC.errors.stop_failed, "kilo_code_serve_stop_failed");
        assert!(SPEC.reserved_ports.contains(&4096));
        assert!(SPEC.reserved_ports.contains(&24173));
        // The preferred port is never one this Agent reserved for itself.
        assert!(!SPEC.reserved_ports.contains(&SPEC.default_port));
        assert_eq!(
            SPEC.executable_environment,
            &["KILO_BIN", "KILO_PATH", "KILOCODE_PATH"]
        );
    }

    #[test]
    fn one_attach_url_joins_every_document_path_without_doubling_separators() {
        assert_eq!(
            endpoint_url("http://127.0.0.1:4097", SPEC.health_path),
            "http://127.0.0.1:4097/global/health"
        );
        assert_eq!(
            endpoint_url("http://127.0.0.1:4097/", SPEC.session_probe_path),
            "http://127.0.0.1:4097/session"
        );
    }
}
