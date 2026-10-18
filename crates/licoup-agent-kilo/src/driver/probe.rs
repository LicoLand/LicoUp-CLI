//! The capability probe this Agent answers before it is offered.
//!
//! A host asks one thing before it offers Kilo Code: is this endpoint up, and
//! which models is it on? The probe answers by reading this Agent's own four
//! documents — health, sessions, configuration and providers — and reporting
//! readiness only when all four were understood. Anything less reports a failure
//! with the code that names what was missing.
//!
//! The loop is here because the *waiting* is this Agent's contract: the endpoint
//! may take up to a minute to become healthy, and a probe that gave up early
//! would report a healthy endpoint as missing. The sockets, however, are the
//! engine's — every read arrives through [`crate::port::serve`], so the package
//! never opens one.
//!
//! What the host composes is the same probe in the host's own result vocabulary
//! ([`capability_probe`]), and that crossing is one way: the failure keeps this
//! Agent's message and stage exactly, and its code is re-stated from the closed
//! set this package's policy declares, because the host's failure type carries a
//! static code. A code outside that set is reported as a protocol failure rather
//! than spliced in as an arbitrary string.

use super::DRIVER;
use super::projection::serve_capabilities;
use super::{ProtocolFailure, RUNTIME_PROTOCOL};
use crate::parser;
use crate::policy;
use crate::port::serve;
use licoup_agent_adapter_sdk::serve::ServeReadiness;
use licoup_agent_drivers::acp_driver_runtime::{
    CapabilityProbe as DriverCapabilityProbe, ProtocolFailure as DriverProtocolFailure,
};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

/// How long the probe waits between two health reads.
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The engine operations one probe performs.
///
/// It carries the two facts the engine owns — where a diagnostic record's bytes
/// are filed and whether a path is absolute — so a probe can be exercised
/// without a host at all.
#[derive(Clone, Copy)]
pub struct EndpointProbe {
    pub get_json: fn(&str) -> Result<serde_json::Value, String>,
    pub is_absolute: fn(&Path) -> bool,
}

impl EndpointProbe {
    /// The probe an installed host answers with.
    ///
    /// Readiness reads are ordinary documents: they are not part of a turn's
    /// diagnostic record, so none of them asks the engine to observe bytes.
    pub fn installed() -> Self {
        Self {
            get_json: |url| serve::get_json(url, false),
            is_absolute: Path::is_absolute,
        }
    }
}

/// This Agent's endpoint paths, read from its own policy.
fn health_url(attach_url: &str) -> String {
    policy::endpoint_url(attach_url, policy::SPEC.health_path)
}

fn sessions_url(attach_url: &str) -> String {
    policy::endpoint_url(attach_url, policy::SPEC.session_probe_path)
}

/// Probe one endpoint, reporting what this Agent's documents said.
///
/// `executable` names the program the engine attaches to; `cwd` is the directory
/// this Agent requires to be absolute; `timeout_ms` bounds the wait for health.
pub fn probe_endpoint(
    executable: &str,
    cwd: &Path,
    timeout_ms: u64,
    probe: EndpointProbe,
) -> Result<ServeReadiness, ProtocolFailure> {
    if !(probe.is_absolute)(cwd) {
        return Err(ProtocolFailure::new(
            "acp_working_directory_invalid",
            "ACP conversation sessions require an absolute working directory.",
            "initialize",
        ));
    }
    if executable.trim().is_empty() {
        return Err(unavailable_failure());
    }
    let attachment =
        serve::ensure_attachment(executable).map_err(|error_code| endpoint_failure(&error_code))?;
    let attach_url = &attachment.endpoint.attach_url;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(1_000));
    loop {
        match (probe.get_json)(&health_url(attach_url)) {
            Ok(health) if parser::health_ready(&health) => {
                let sessions = (probe.get_json)(&sessions_url(attach_url)).map_err(|_| {
                    ProtocolFailure::new(
                        "acp_initialize_invalid",
                        "The ACP agent returned an invalid initialization response.",
                        "serve/session",
                    )
                })?;
                if !parser::session_collection(&sessions) {
                    return Err(ProtocolFailure::new(
                        "kilo_code_serve_session_invalid",
                        "The Kilo session endpoint returned an invalid response.",
                        "serve/session",
                    ));
                }
                // The configuration and provider documents decide the model
                // catalogue. A service that answers health but not these is not
                // ready: a turn started against it could not resolve its model.
                let config =
                    (probe.get_json)(&policy::endpoint_url(attach_url, policy::SPEC.config_path))
                        .map_err(|_| readiness_unavailable("serve/config"))?;
                let providers = (probe.get_json)(&policy::endpoint_url(
                    attach_url,
                    policy::SPEC.provider_path,
                ))
                .map_err(|_| readiness_unavailable("serve/provider"))?;
                return parser::readiness(&health, &sessions, &config, &providers)
                    .ok_or_else(|| readiness_unavailable("serve/readiness"));
            }
            _ if Instant::now() >= deadline => {
                return Err(ProtocolFailure::new(
                    "acp_protocol_timeout",
                    "The ACP agent timed out during capability negotiation.",
                    "serve/health",
                ));
            }
            _ => thread::sleep(HEALTH_POLL_INTERVAL),
        }
    }
}

fn readiness_unavailable(stage: &'static str) -> ProtocolFailure {
    ProtocolFailure::new(
        "kilo_code_serve_readiness_unavailable",
        "The Kilo serve endpoint did not report the documents a turn needs.",
        stage,
    )
}

/// Probe one endpoint in the host's shared driver vocabulary.
///
/// It is the same probe as [`probe_endpoint`], which owns both the readiness
/// read and the capability declaration: the host runs no second probe and
/// declares no capability of its own, and what crosses here is the outcome and
/// the failure, not a vendor decision.
pub fn capability_probe(
    executable: &str,
    cwd: &Path,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    max_stderr: usize,
) -> Result<DriverCapabilityProbe, DriverProtocolFailure> {
    let _ = (max_stdout, max_stderr);
    probe_endpoint(executable, cwd, timeout_ms, EndpointProbe::installed())
        .map(|_readiness| probe_to_driver(serve_capabilities()))
        .map_err(failure_to_driver)
        .map_err(|failure| failure.namespaced(DRIVER))
}

/// This Agent's failure, in the host's shared driver vocabulary.
///
/// The code is re-stated from this Agent's own closed set rather than carried
/// across as a string, because the host's failure type holds a static spelling.
pub(super) fn failure_to_driver(failure: ProtocolFailure) -> DriverProtocolFailure {
    DriverProtocolFailure::new(static_code(&failure.code), failure.message, failure.stage)
        .with_session(failure.session_id.as_deref())
}

/// This Agent's static spelling of one of its own closed failure codes.
///
/// A code outside the set is reported as a protocol failure rather than spliced
/// in as an arbitrary string.
fn static_code(code: &str) -> &'static str {
    let errors = &policy::SPEC.errors;
    for known in [
        errors.executable_missing,
        errors.port_exhausted,
        errors.start_failed,
        errors.health_failed,
        errors.attach_probe_failed,
        errors.not_found,
        errors.request_failed,
        errors.invalid_json,
        errors.invalid_state,
        errors.stop_failed,
        "acp_working_directory_invalid",
        "acp_initialize_invalid",
        "acp_protocol_timeout",
        "acp_process_start_failed",
        "kilo_code_serve_session_invalid",
        "kilo_code_serve_readiness_unavailable",
    ] {
        if code == known {
            return known;
        }
    }
    "kilo_code_serve_protocol_failed"
}

/// This Agent's capability answer, in the host's shared probe vocabulary.
///
/// It is a field copy: the package already decided every capability, and this
/// function decides none of them.
fn probe_to_driver(probe: super::projection::CapabilityProbe) -> DriverCapabilityProbe {
    DriverCapabilityProbe {
        protocol_version: probe.protocol_version,
        load_session: probe.load_session,
        resume_session: probe.resume_session,
        close_session: probe.close_session,
        list_sessions: probe.list_sessions,
        delete_session: probe.delete_session,
        additional_directories: probe.additional_directories,
        image_prompts: probe.image_prompts,
        audio_prompts: probe.audio_prompts,
        embedded_context: probe.embedded_context,
    }
}

/// The failure one missing executable reports.
pub fn unavailable_failure() -> ProtocolFailure {
    ProtocolFailure::new(
        "acp_process_start_failed",
        "The requested ACP agent executable is not available.",
        "serve/ensure",
    )
}

/// The failure one engine code means, in this Agent's own vocabulary.
///
/// Every code the Agent's own policy can report has a fixed message and stage
/// here; a code this package does not recognise is reported as a start failure
/// rather than passed through as an unexplained string.
pub fn endpoint_failure(error_code: &str) -> ProtocolFailure {
    let errors = policy::SPEC.errors;
    match error_code.trim() {
        code if code == errors.executable_missing => ProtocolFailure::new(
            errors.executable_missing,
            "The requested Kilo executable is not available.",
            "serve/ensure",
        ),
        code if code == errors.port_exhausted => ProtocolFailure::new(
            errors.port_exhausted,
            "No local port is available for the Kilo serve endpoint.",
            "serve/ensure",
        ),
        code if code == errors.start_failed => ProtocolFailure::new(
            errors.start_failed,
            "The Kilo serve process could not be started.",
            "serve/ensure",
        ),
        code if code == errors.health_failed => ProtocolFailure::new(
            errors.health_failed,
            "The Kilo serve endpoint did not become healthy.",
            "serve/health",
        ),
        code if code == errors.attach_probe_failed => ProtocolFailure::new(
            errors.attach_probe_failed,
            "The Kilo serve endpoint rejected the attach probe.",
            "serve/session",
        ),
        code if code == errors.invalid_state => ProtocolFailure::new(
            errors.invalid_state,
            "The Kilo serve state is invalid.",
            "serve/ensure",
        ),
        _ => ProtocolFailure::new(
            "acp_process_start_failed",
            "The Kilo serve endpoint is not available for attach.",
            "serve/ensure",
        ),
    }
}

/// The absolute path this Agent requires, or the failure that says so.
pub fn require_absolute(cwd: &Path) -> Result<PathBuf, ProtocolFailure> {
    if cwd.is_absolute() {
        Ok(cwd.to_path_buf())
    } else {
        Err(ProtocolFailure::new(
            "acp_working_directory_invalid",
            "ACP conversation sessions require an absolute working directory.",
            "initialize",
        ))
    }
}

/// The runtime protocol stamp this Agent's results carry.
pub fn runtime_protocol() -> &'static str {
    RUNTIME_PROTOCOL
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A probe that reads nothing: the refusals below happen before any read,
    /// which is exactly what they assert.
    fn silent_probe() -> EndpointProbe {
        EndpointProbe {
            get_json: |_| Err("the probe must not read before it is ready".to_owned()),
            is_absolute: Path::is_absolute,
        }
    }

    #[test]
    fn a_relative_directory_is_refused_before_the_endpoint_is_touched() {
        let probe = silent_probe();
        let failure = probe_endpoint("kilo", Path::new("relative"), 10, probe).unwrap_err();
        assert_eq!(failure.code, "acp_working_directory_invalid");
    }

    #[test]
    fn an_empty_executable_reports_the_unavailable_failure() {
        let probe = silent_probe();
        let failure = probe_endpoint("  ", Path::new("/workspace"), 10, probe).unwrap_err();
        assert_eq!(failure.code, "acp_process_start_failed");
        assert_eq!(failure.stage, "serve/ensure");
    }

    #[test]
    fn every_policy_code_has_its_own_message_and_an_unknown_code_falls_back() {
        let errors = policy::SPEC.errors;
        for code in [
            errors.executable_missing,
            errors.port_exhausted,
            errors.start_failed,
            errors.health_failed,
            errors.attach_probe_failed,
            errors.invalid_state,
        ] {
            let failure = endpoint_failure(code);
            assert_eq!(failure.code, code);
            assert!(!failure.message.is_empty());
        }
        let unknown = endpoint_failure("something_else");
        assert_eq!(unknown.code, "acp_process_start_failed");
        assert_eq!(unknown.stage, "serve/ensure");
    }

    #[test]
    fn readiness_requires_all_four_documents() {
        // Health and sessions are healthy, but the provider catalogue is empty,
        // so no current model exists and the endpoint is not ready.
        assert!(
            parser::readiness(
                &json!({"healthy": true, "version": "1.2.3"}),
                &json!([]),
                &json!({}),
                &json!({"all": []}),
            )
            .is_none()
        );
    }

    #[test]
    fn require_absolute_answers_the_path_or_the_refusal() {
        assert_eq!(
            require_absolute(Path::new("/workspace")).unwrap(),
            PathBuf::from("/workspace")
        );
        assert!(require_absolute(Path::new("workspace")).is_err());
        assert_eq!(runtime_protocol(), "kilo-code-serve-http-v1");
    }

    #[test]
    fn a_relative_workspace_is_refused_before_any_process_or_http_work() {
        let failure = capability_probe("unused", Path::new("relative"), 10, None, 16).unwrap_err();
        // The refusal keeps the closed set's spelling and then carries this
        // Agent's own error prefix, which is what the host's failure taxonomy
        // reads it by.
        assert_eq!(failure.code, "kilo_code_serve_working_directory_invalid");
        assert_eq!(failure.stage, "initialize");
    }

    #[test]
    fn an_empty_executable_reports_the_process_start_failure() {
        let failure = capability_probe("  ", Path::new("/workspace"), 10, None, 16).unwrap_err();
        assert_eq!(failure.code, "kilo_code_serve_process_start_failed");
        assert_eq!(failure.stage, "serve/ensure");
    }

    #[test]
    fn every_package_code_keeps_its_own_spelling_across_the_seam() {
        let errors = &policy::SPEC.errors;
        for code in [
            errors.executable_missing,
            errors.port_exhausted,
            errors.start_failed,
            errors.health_failed,
            errors.attach_probe_failed,
            errors.invalid_state,
        ] {
            assert_eq!(static_code(code), code);
        }
        // A code outside the closed set is reported as a protocol failure rather
        // than spliced in as an arbitrary string.
        assert_eq!(
            static_code("something_unexpected"),
            "kilo_code_serve_protocol_failed"
        );
    }

    #[test]
    fn the_package_failure_crosses_with_its_message_stage_and_session_intact() {
        let failure = failure_to_driver(
            ProtocolFailure::new(
                "kilo_code_serve_session_invalid",
                "The Kilo session endpoint returned an invalid response.",
                "serve/session",
            )
            .with_session(Some("kilo-1")),
        );
        assert_eq!(failure.code, "kilo_code_serve_session_invalid");
        assert_eq!(failure.stage, "serve/session");
        assert_eq!(
            failure.message,
            "The Kilo session endpoint returned an invalid response."
        );
        assert_eq!(failure.session_id.as_deref(), Some("kilo-1"));
        assert_eq!(failure.thread_id.as_deref(), Some("kilo-1"));
    }

    #[test]
    fn the_capability_answer_crosses_as_a_field_copy() {
        let declared = serve_capabilities();
        let probe = probe_to_driver(declared);
        assert_eq!(probe.protocol_version, declared.protocol_version);
        assert_eq!(probe.load_session, declared.load_session);
        assert_eq!(probe.resume_session, declared.resume_session);
        assert_eq!(probe.close_session, declared.close_session);
        assert_eq!(probe.list_sessions, declared.list_sessions);
        assert_eq!(probe.delete_session, declared.delete_session);
        assert_eq!(
            probe.additional_directories,
            declared.additional_directories
        );
        assert_eq!(probe.image_prompts, declared.image_prompts);
        assert_eq!(probe.audio_prompts, declared.audio_prompts);
        assert_eq!(probe.embedded_context, declared.embedded_context);
        assert_eq!(probe.protocol_version, Some(1));
        assert!(probe.load_session && probe.resume_session && probe.list_sessions);
        assert!(!probe.delete_session && !probe.image_prompts && !probe.audio_prompts);
    }
}
