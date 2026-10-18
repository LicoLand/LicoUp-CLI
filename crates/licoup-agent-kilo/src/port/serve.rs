//! The shared local-service engine, as one installed port.
//!
//! Kilo Code is one of the Agents LicoUp drives through a loopback HTTP service
//! the Agent's own program runs. Starting and supervising that program, reading
//! its documents over HTTP, framing its SSE stream, recording raw bytes for a
//! diagnostic record and admitting an active turn so force stop can reach it are
//! *the same work for every serve-family Agent* — that is the client's engine,
//! and it stays the client's.
//!
//! What is not the same, and therefore arrives through this seam, is what one
//! Kilo turn asks the engine for: which documents it reads, in what order, and
//! what a failure of each means. The package owns that; the engine owns the
//! sockets, the process and the registry.
//!
//! Until a host installs the port every member is fail-closed: no endpoint, no
//! document and no admission. A package running outside the client reports that
//! it could not ask rather than inventing an attachment, which is what keeps the
//! parser and the replay corpus exercisable with no host at all.

use serde_json::Value;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;

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

/// What one successful attachment to the Agent's service reports.
#[derive(Clone, Debug)]
pub struct ServeAttachment {
    pub endpoint: ServeEndpoint,
    pub catalog: licoup_agent_adapter_sdk::serve::ServeModelCatalog,
}

/// Why an installed host stopped reading a stream.
///
/// The framing variants are the engine's own closed vocabulary: this package
/// names the failure codes, the engine names which framing limit was hit, and
/// neither invents the other's answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServeStreamFailure {
    /// The stream ended before the turn completed.
    Closed,
    /// A stream document did not decode.
    Decode,
    /// The engine's framing refused a frame.
    Framing(ServeFramingFailure),
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

/// The engine's active-turn registration, held for as long as one turn runs.
///
/// The registration belongs to the engine: force stop reaches an active turn
/// through the record this guard keeps alive, and dropping the guard is how the
/// turn gives that record up. The package holds it and reads nothing from it,
/// which is why the engine's own registration type is carried rather than named.
pub struct ServeTurnGuard {
    _held: Box<dyn Send>,
}

impl ServeTurnGuard {
    /// Hold one engine registration for as long as this guard lives.
    pub fn hold(registration: impl Send + 'static) -> Self {
        Self {
            _held: Box::new(registration),
        }
    }
}

/// The host facilities one Kilo turn needs from the serve engine.
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
    pub get_json: fn(url: &str, observed: bool) -> Result<Value, String>,
    /// Write one document to the endpoint.
    pub post_json: fn(url: &str, body: &Value) -> Result<Value, String>,
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
    /// A host with no record ignores the call, which is the same shape of answer
    /// as a return the host has no consumer for.
    pub observe_bytes:
        fn(source: &str, direction: ServeByteDirection, session_id: Option<&str>, bytes: &str),
    /// Register one active turn so force stop can reach it, and hold that
    /// registration for as long as the returned guard lives.
    ///
    /// `None` is the engine's refusal — the active-turn registry is at capacity,
    /// or this process never installed a serve engine — and a refused turn never
    /// runs: a turn force stop cannot reach is a turn this client cannot stop.
    pub register_turn: fn(attach_url: &str, session_id: &str) -> Option<ServeTurnGuard>,
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

/// The serve engine this process installed, or `None` before composition.
///
/// It answers the one installed value rather than a copy of it, so a caller
/// holding this port and a caller reaching the module's own accessors cannot
/// disagree about which engine a turn runs on.
pub fn port() -> Option<ServePort> {
    PORT.get().copied()
}

/// Attach to this Agent's service, or report that no host answered.
pub fn ensure_attachment(executable: &str) -> Result<ServeAttachment, String> {
    PORT.get()
        .ok_or_else(|| "the serve port is not installed".to_owned())
        .and_then(|port| (port.ensure_attachment)(executable))
}

pub(crate) fn get_json(url: &str, observed: bool) -> Result<Value, String> {
    PORT.get()
        .ok_or_else(|| "the serve port is not installed".to_owned())
        .and_then(|port| (port.get_json)(url, observed))
}

pub(crate) fn post_json(url: &str, body: &Value) -> Result<Value, String> {
    PORT.get()
        .ok_or_else(|| "the serve port is not installed".to_owned())
        .and_then(|port| (port.post_json)(url, body))
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
    bytes: &str,
) {
    if let Some(port) = PORT.get() {
        (port.observe_bytes)(source, direction, session_id, bytes);
    }
}
