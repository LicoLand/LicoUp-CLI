//! The shared local-service engine, as one installed port.
//!
//! OpenCode is one of the Agents LicoUp drives through a loopback HTTP service
//! the Agent's own program runs. Starting and supervising that program, reading
//! its documents over HTTP, framing its SSE stream, recording raw bytes for a
//! diagnostic record and admitting an active turn so force stop can reach it are
//! *the same work for every serve-family Agent* — that is the client's engine,
//! and it stays the client's.
//!
//! What is not the same, and therefore arrives through this seam, is what one
//! OpenCode turn asks the engine for: which documents it reads, in what order,
//! how long a turn's remaining budget allows one of those reads, and what a
//! failure of each means. The package owns that; the engine owns the sockets,
//! the process and the registry.
//!
//! Until a host installs the port every member is fail-closed: no endpoint, no
//! document, no stream and no admission. A package running outside the client
//! reports that it could not ask rather than inventing an attachment, which is
//! what keeps the parser and the replay corpus exercisable with no host at all.

use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use licoup_agent_adapter_sdk::serve::ServeModelCatalog;
use serde_json::Value;

/// One attachable endpoint of this Agent's local service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServeEndpoint {
    pub host: String,
    pub port: u16,
    pub attach_url: String,
}

impl ServeEndpoint {
    /// The endpoint one host and port describe.
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        let host = host.into();
        Self {
            attach_url: format!("http://{host}:{port}"),
            host,
            port,
        }
    }
}

/// The engine's hold on an attached endpoint, kept alive with the attachment.
///
/// The engine owns what a lease is; this package only keeps it alive for as long
/// as it holds the attachment, which is what keeps force stop able to find the
/// endpoint for the whole turn. A host that pins nothing answers with none.
pub struct ServeAttachmentLease(
    /// Held only to be dropped with the lease; the engine owns what it means.
    #[allow(dead_code)]
    Option<Box<dyn std::any::Any + Send + Sync>>,
);

impl ServeAttachmentLease {
    /// Hold one host value for as long as this lease lives.
    pub fn held<T: Send + Sync + 'static>(value: T) -> Self {
        Self(Some(Box::new(value)))
    }

    /// An attachment no engine pinned.
    pub const fn unpinned() -> Self {
        Self(None)
    }
}

/// What one successful attachment to the Agent's service reports.
pub struct ServeAttachment {
    pub endpoint: ServeEndpoint,
    pub catalog: ServeModelCatalog,
    /// Kept alive with the attachment and dropped with it.
    pub lease: ServeAttachmentLease,
}

/// One HTTP failure of the engine's bounded reader, in the engine's own closed
/// vocabulary.
///
/// The variants are the engine's; the codes this Agent reports for them are the
/// package's (`crate::driver::request_failure`). Neither invents the other's
/// answer, which is why the crossing is a field copy rather than a string a
/// reader would have to parse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServeRequestFailure {
    BodyTooLarge,
    Busy,
    HeadersTooLarge,
    InvalidJson,
    InvalidUrl,
    NotFound,
    Request,
    Serialize,
    /// The endpoint answered with a non-success status.
    Status(u16),
    Unavailable,
}

/// The framing limits one engine can report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServeFramingFailure {
    Busy,
    EventLimit,
    FrameTooLarge,
    HeadersTooLarge,
    InvalidUtf8,
    InvalidUrl,
    LineTooLarge,
    Request,
    Unavailable,
}

/// The direction of one observed byte range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServeByteDirection {
    Received,
    Sent,
}

/// Whether the engine admitted an active turn for one session.
pub enum ServeTurnAdmission {
    /// Force stop can now reach this session's turn for as long as the guard
    /// lives.
    Admitted(ServeTurnGuard),
    /// The engine's active-turn registry is at capacity.
    AtCapacity,
}

/// One admitted active turn, held for as long as the turn runs.
///
/// The engine owns the registration; this package owns only how long it lasts,
/// which is the whole turn. Dropping the guard releases it.
pub struct ServeTurnGuard(
    /// Held only to be dropped with the guard; the engine owns the registration.
    #[allow(dead_code)]
    Option<Box<dyn std::any::Any + Send + Sync>>,
);

impl ServeTurnGuard {
    /// Hold one host registration for as long as this guard lives.
    pub fn held<T: Send + Sync + 'static>(value: T) -> Self {
        Self(Some(Box::new(value)))
    }

    /// An admission no engine registered.
    pub const fn unregistered() -> Self {
        Self(None)
    }
}

/// The one engine answer a control failure is reported through.
///
/// The engine reports the failure its own control request observed; the package
/// decides what that means for the turn, so the callback is stated in the
/// package's own failure vocabulary.
pub type ServeControlFailureObserver =
    std::sync::Arc<dyn Fn(ServeRequestFailure) + Send + Sync + 'static>;

/// The host facilities one OpenCode turn needs from the serve engine.
///
/// Each member is one answer the host owns. They arrive as one installed value
/// so a package cannot be half-wired: either the host answered the port or the
/// package is fail-closed as a whole.
#[derive(Clone, Copy)]
pub struct ServePort {
    /// Attach to this Agent's service, starting it when it is not already up.
    /// `Err` carries the engine's own stable failure code.
    pub ensure_attachment: fn(executable: &str) -> Result<ServeAttachment, String>,
    /// Read one document from the endpoint.
    ///
    /// `observed` asks the engine to file this read in its diagnostic record;
    /// the package decides *which* reads are worth recording, the engine decides
    /// where they go.
    pub get_json: fn(url: &str, observed: bool) -> Result<Value, ServeRequestFailure>,
    /// Write one document to the endpoint, bounded by the turn's remaining
    /// budget.
    ///
    /// `timeout` is the turn deadline the package computed — an OpenCode turn's
    /// contract, not the engine's — and `None` means the turn opted out of a
    /// deadline rather than that the engine should pick one.
    pub post_json: fn(
        url: &str,
        body: &Value,
        timeout: Option<Duration>,
    ) -> Result<Value, ServeRequestFailure>,
    /// Read the endpoint's event stream until `stop`, offering each decoded
    /// frame to `on_frame` as its data payload and its whole framed text.
    ///
    /// `on_frame` answers whether to keep reading. The engine performs the
    /// framing; the package decides what a frame means and whether the turn is
    /// over.
    pub watch_frames: fn(
        url: &str,
        stop: &AtomicBool,
        on_frame: &mut dyn FnMut(&str, &str) -> bool,
    ) -> Result<(), ServeFramingFailure>,
    /// Record one observed byte range for the diagnostic record, when the host
    /// keeps one and when `session_id` decides this frame belongs to the turn.
    ///
    /// `payload` is the frame's data — what the classification reads — and
    /// `frame` is the whole framed text the record keeps. Ownership is decided
    /// from the payload because that is the engine's own rule, and the package
    /// never re-derives it. A host with no record ignores the call, which is the
    /// same shape of answer as a return the host has no consumer for.
    pub observe_bytes: fn(
        source: &str,
        direction: ServeByteDirection,
        session_id: Option<&str>,
        payload: &str,
        frame: &str,
    ),
    /// Admit one active turn so force stop can reach it, reporting any control
    /// failure it observes through `on_failure` for as long as the guard lives.
    pub admit_turn: fn(
        attach_url: &str,
        session_id: &str,
        on_failure: Option<ServeControlFailureObserver>,
    ) -> ServeTurnAdmission,
}

static PORT: OnceLock<ServePort> = OnceLock::new();

/// Install the host's serve engine once per process.
pub fn install(port: ServePort) -> Result<(), &'static str> {
    PORT.set(port)
        .map_err(|_| "the serve port is already installed")
}

/// Whether the host has installed its serve engine.
pub fn installed() -> bool {
    PORT.get().is_some()
}

/// Attach to this Agent's service, or report that no host answered.
pub fn ensure_attachment(executable: &str) -> Result<ServeAttachment, String> {
    PORT.get()
        .ok_or_else(|| "the serve port is not installed".to_owned())
        .and_then(|port| (port.ensure_attachment)(executable))
}

pub(crate) fn get_json(url: &str, observed: bool) -> Result<Value, ServeRequestFailure> {
    PORT.get()
        .ok_or(ServeRequestFailure::Unavailable)
        .and_then(|port| (port.get_json)(url, observed))
}

pub(crate) fn post_json(
    url: &str,
    body: &Value,
    timeout: Option<Duration>,
) -> Result<Value, ServeRequestFailure> {
    PORT.get()
        .ok_or(ServeRequestFailure::Unavailable)
        .and_then(|port| (port.post_json)(url, body, timeout))
}

pub(crate) fn watch_frames(
    url: &str,
    stop: &AtomicBool,
    on_frame: &mut dyn FnMut(&str, &str) -> bool,
) -> Result<(), ServeFramingFailure> {
    match PORT.get() {
        Some(port) => (port.watch_frames)(url, stop, on_frame),
        // No engine: the stream is unavailable, which is the engine's own
        // vocabulary for "nothing answered" rather than a new package code.
        None => Err(ServeFramingFailure::Unavailable),
    }
}

pub(crate) fn observe_bytes(
    source: &str,
    direction: ServeByteDirection,
    session_id: Option<&str>,
    payload: &str,
    frame: &str,
) {
    if let Some(port) = PORT.get() {
        (port.observe_bytes)(source, direction, session_id, payload, frame);
    }
}

pub(crate) fn admit_turn(
    attach_url: &str,
    session_id: &str,
    on_failure: Option<ServeControlFailureObserver>,
) -> ServeTurnAdmission {
    match PORT.get() {
        Some(port) => (port.admit_turn)(attach_url, session_id, on_failure),
        // Fail-closed: a package that cannot ask the engine's admission never
        // claims it was admitted.
        None => ServeTurnAdmission::AtCapacity,
    }
}
