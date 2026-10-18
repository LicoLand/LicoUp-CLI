//! The port this crate's ACP transport engines read one Agent's frame dialect
//! through.
//!
//! This crate owns the shared ACP dialect: the framing that feeds a session,
//! the protocol reducers `acp_driver_runtime::protocol` and
//! `acp_session_transport::protocol` drive, the pooled transport, the session
//! lifecycle and the local service control plane. It owns no Agent's protocol
//! policy. Everything the shared dialect needs *about* one Agent's frames —
//! which byte-line decoder reads them, whether a frame is a notification or a
//! response to a known request, what a session update or a prompt result says,
//! and how a failure is turned into the shared transition vocabulary — arrives
//! through [`AcpParserRegistration`], and the composition above this crate
//! supplies the registrations: `licoup-native` while the Agent crates do not
//! exist, `licoup-agent-<agent>` once they do.
//!
//! The transport is keyed by **driver identity**, because that is the identity
//! the transport already pools and cancels sessions by: one entry answers for
//! every ACP session reached through one driver. Several Agents legitimately
//! share one dialect — Copilot, OpenCode and Kilo Code speak the same ACP
//! profile today — and they select it by declaring the same `driver_id`, or by
//! answering with the same functions. A registration therefore does not name an
//! Agent, and nothing in this module is a vendor branch.
//!
//! Every member is a `fn` pointer rather than a trait, so this crate keeps no
//! state a caller did not hand it. A build that installs no registration
//! answers fail-closed — nothing decodes, nothing completes, nothing
//! validates — rather than inheriting a guess, so a capability that is absent
//! is reported as absent and never as a malformed frame.

use licoup_agent_adapter_sdk::Transition;
use licoup_foundation::core::acp::{self, AcpInitializeResponse, AcpSessionUpdate, AcpStopReason};
use serde_json::Value;

/// A client request one Agent's frame dialect reports.
///
/// The transport reads these four facts and no others: what to answer, which
/// method was asked, which session it belongs to, and which option id the frame
/// offered for a one-time approval. The shape is this transport's own, so an
/// Agent's parser projects onto it rather than this crate naming the parser's
/// type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolClientRequest {
    /// The JSON request id this request must be answered on.
    pub id: Value,
    /// The ACP method the Agent asked the client to answer.
    pub method: String,
    /// The session the request belongs to, when the frame names one.
    pub session_id: Option<String>,
    /// The option id the frame offers for a one-time approval, when it offers
    /// exactly one.
    pub allow_once_option: Option<String>,
}

/// A permission request one Agent's frame dialect reports.
///
/// The transport turns these facts into its own
/// `ProtocolEffect::AwaitExternalApproval` and answers on `id`; it never
/// re-derives the summary or the tool list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolPermissionRequest {
    /// The JSON request id this permission question must be answered on.
    pub id: Value,
    /// The ACP method the Agent asked the client to answer.
    pub method: String,
    /// The session the question belongs to, when the frame names one.
    pub session_id: Option<String>,
    /// The redacted summary the transport shows a user.
    pub display_summary: String,
    /// The single option id the frame offers, when it offers exactly one.
    pub option_id: Option<String>,
    /// The tools the Agent is asking permission to use.
    pub requested_tools: Vec<String>,
}

/// Decode one raw byte line into a frame, or report why it is not one.
pub type ProtocolDecodeFrame = fn(&[u8]) -> Result<Value, acp::AcpError>;

/// Whether a frame is a notification rather than a response.
pub type ProtocolIsNotification = fn(&Value) -> bool;

/// Whether a frame is the response to the request carrying `expected`.
pub type ProtocolResponseIdMatches = fn(&Value, i64) -> bool;

/// Whether a frame reports a protocol-level error.
pub type ProtocolResponseIsError = fn(&Value) -> bool;

/// Read one session update out of a frame.
pub type ProtocolSessionUpdate =
    fn(&Value, Option<&str>) -> Result<AcpSessionUpdate, acp::AcpError>;

/// Read one prompt result's stop reason out of a frame.
pub type ProtocolPromptStopReason = fn(&Value, i64) -> Result<AcpStopReason, acp::AcpError>;

/// Read the initialize response out of a raw byte line, when the line is it.
pub type ProtocolInitializeResponse =
    fn(&[u8], i64) -> Result<Option<AcpInitializeResponse>, acp::AcpError>;

/// Read one client request out of a frame, when the frame is one.
pub type ProtocolClientRequestReader = fn(&Value) -> Option<ProtocolClientRequest>;

/// Read one permission request out of a frame, when the frame is one.
pub type ProtocolPermissionRequestReader = fn(&Value) -> Option<ProtocolPermissionRequest>;

/// Turn one completed turn into the shared transition vocabulary.
pub type ProtocolCompletedTransitions = fn(&str) -> Vec<Transition>;

/// Turn one protocol failure into the shared transition vocabulary.
pub type ProtocolFailedTransitions = fn(&str, &str, &str) -> Vec<Transition>;

/// One Agent's frame dialect, as this crate's ACP transport engines read it.
///
/// The members are the whole of what the shared dialect may not know by name.
/// A dialect that cannot answer one of them is not an ACP dialect, so all
/// eleven are required rather than defaulted.
#[derive(Clone, Copy, Debug)]
pub struct AcpParserRegistration {
    /// The driver identity the transport pools and cancels sessions by. This is
    /// the identity the transport is already keyed on, so a registration is
    /// reachable from every site that holds a driver spec.
    pub driver_id: &'static str,
    /// Decode one raw byte line into a frame.
    pub decode_frame: ProtocolDecodeFrame,
    /// Whether a frame is a notification.
    pub is_notification: ProtocolIsNotification,
    /// Whether a frame is the response to a given request id.
    pub response_id_matches: ProtocolResponseIdMatches,
    /// Whether a frame reports a protocol-level error.
    pub response_is_error: ProtocolResponseIsError,
    /// Read one session update out of a frame.
    pub session_update: ProtocolSessionUpdate,
    /// Read one prompt result's stop reason out of a frame.
    pub prompt_stop_reason: ProtocolPromptStopReason,
    /// Read the initialize response out of a raw byte line.
    pub initialize_response: ProtocolInitializeResponse,
    /// Read one client request out of a frame.
    pub client_request: ProtocolClientRequestReader,
    /// Read one permission request out of a frame.
    pub permission_request: ProtocolPermissionRequestReader,
    /// The transitions this dialect reports for a completed turn.
    pub completed_transitions: ProtocolCompletedTransitions,
    /// The transitions this dialect reports for a failed turn.
    pub failed_transitions: ProtocolFailedTransitions,
}

/// A dialect that answers nothing.
///
/// Every reader refuses and every decoder reports an error, so a transport that
/// reached an unregistered driver reports a protocol failure rather than
/// silently treating a frame as valid. The identity is empty, which no driver
/// declares, so it can never be looked up by accident. The codes it reports are
/// the shared ACP error codes every decoder already uses, so an uncomposed
/// build fails through the same projection a malformed frame does.
static UNAVAILABLE: AcpParserRegistration = AcpParserRegistration {
    driver_id: "",
    decode_frame: |_| Err(acp::AcpError::JsonLineInvalid),
    is_notification: |_| false,
    response_id_matches: |_, _| false,
    response_is_error: |_| false,
    session_update: |_, _| Err(acp::AcpError::ResponseEnvelopeInvalid),
    prompt_stop_reason: |_, _| Err(acp::AcpError::ResponseEnvelopeInvalid),
    initialize_response: |_, _| Err(acp::AcpError::ResponseEnvelopeInvalid),
    client_request: |_| None,
    permission_request: |_| None,
    completed_transitions: |_| Vec::new(),
    failed_transitions: |_, _, _| Vec::new(),
};

/// The dialect this crate reads an unregistered driver through. It answers
/// fail-closed, and it is what a build that installed no composition gets.
pub const fn unavailable_parser() -> AcpParserRegistration {
    UNAVAILABLE
}

/// Look one driver's dialect up out of a composed set.
///
/// The lookup is part of this module rather than each caller so the
/// fail-closed answer is stated once: a driver the composition does not
/// register reads the unavailable dialect, never another Agent's.
pub fn parser_for_driver(
    registrations: &[AcpParserRegistration],
    driver_id: &str,
) -> AcpParserRegistration {
    registrations
        .iter()
        .copied()
        .find(|registration| registration.driver_id == driver_id)
        .unwrap_or(UNAVAILABLE)
}

/// The dialects the composition above this crate installed.
static PARSERS: std::sync::OnceLock<Vec<AcpParserRegistration>> = std::sync::OnceLock::new();

/// Install the dialects this build reads Agents' frames through.
///
/// The first installation wins: a second one is refused rather than silently
/// replacing the dialect a running turn may already be reading, which is the
/// same rule the host's own composition port states.
pub fn install(registrations: Vec<AcpParserRegistration>) -> bool {
    PARSERS.set(registrations).is_ok()
}

/// Every dialect this build installed. A build that installed none reads every
/// driver through the unavailable dialect rather than inheriting a guess.
pub fn installed() -> &'static [AcpParserRegistration] {
    PARSERS.get().map(Vec::as_slice).unwrap_or(&[])
}

/// The dialect one driver's frames are read through, from the installed set.
///
/// This is the accessor the transport engines call: they hold a driver spec and
/// no Agent's name, so the dialect is resolved by the identity the transport is
/// already keyed on.
pub fn parser_for(driver_id: &str) -> AcpParserRegistration {
    parser_for_driver(installed(), driver_id)
}
