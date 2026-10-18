//! C09: the carrier — one line per frame, one protocol on `stdout`, bounded
//! everything.
//!
//! The baseline carrier is a local process speaking JSON-RPC 2.0, one frame per
//! line. `stdout` carries the program protocol and nothing else; diagnostics go
//! to `stderr` and are bounded. A local IPC channel or an already-connected
//! service may be negotiated instead, but the profile is the same one, so an
//! extension does not become a different kind of program because it moved from a
//! pipe to a socket.
//!
//! Three bounds exist because the alternative is a client that appears to hang:
//!
//! - **Frames are bounded.** The initial bound is 64 KiB and it is negotiated
//!   down to whatever both sides accept. A larger payload is chunked, or
//!   referred to by a blob handle the host issued — never by a filesystem path an
//!   extension chose ([`blob_reference_is_controlled`]).
//! - **Logs are bounded.** A carrier that lets a program emit unbounded
//!   `stderr` turns a chatty adapter into a client that stops responding.
//! - **Control is reserved.** A cancellation may not queue behind a very large
//!   data frame. On a single-stream carrier the honest statement is a worst-case
//!   wait ([`Framing::worst_case_control_wait_bytes`]) plus reserved slots, not a
//!   claim that an unsplittable write can be preempted in the middle.

use licoup_application::ApplicationFailure;

use crate::refusal;

/// The value of the `jsonrpc` member of every frame: the carriers speak JSON-RPC
/// 2.0.
pub const JSONRPC_VERSION: &str = "2.0";

/// The frame bound an extension gets when neither side asks for another.
pub const DEFAULT_MAX_FRAME_BYTES: usize = 64 * 1024;

/// The smallest bound a carrier may negotiate to. Below this, ordinary
/// descriptions do not fit and the negotiation is not a negotiation.
pub const MIN_MAX_FRAME_BYTES: usize = 4 * 1024;

/// The largest bound a carrier may negotiate to, so "negotiable" cannot become
/// an unbounded buffer.
pub const MAX_MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// The most bytes one diagnostic line may carry before the carrier truncates it.
pub const MAX_LOG_LINE_BYTES: usize = 8 * 1024;

/// The frames a carrier keeps for control messages while data frames are queued.
pub const RESERVED_CONTROL_SLOTS: usize = 4;

/// The scheme a controlled blob reference uses. The authority is the host: the
/// handle is issued by it, and the extension may only pass it back.
pub const BLOB_SCHEME: &str = "blob:";

/// The longest controlled blob reference accepted.
pub const MAX_BLOB_REFERENCE_BYTES: usize = 128;

/// How an extension is reached.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transport {
    /// A child process of the client, framed over `stdin`/`stdout`.
    StdioProcess,
    /// A local IPC channel — a socket or a pipe the client opened.
    LocalIpc,
    /// An authenticated service endpoint the user configured. It is bridged, not
    /// started, and it reports its own cancel and recovery abilities truthfully.
    ConnectedService,
}

impl Transport {
    pub const fn id(self) -> &'static str {
        match self {
            Self::StdioProcess => "stdio-process",
            Self::LocalIpc => "local-ipc",
            Self::ConnectedService => "connected-service",
        }
    }

    /// Whether the client starts and supervises this transport.
    ///
    /// A connected service is not a child of the client: it is bridged, so
    /// disconnection, recovery and cancellation are facts the remote side
    /// reports, not promises the client can make.
    pub const fn is_supervised(self) -> bool {
        matches!(self, Self::StdioProcess | Self::LocalIpc)
    }
}

/// A negotiated frame bound and the control reservation that goes with it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Framing {
    host_max_frame_bytes: usize,
    extension_max_frame_bytes: usize,
    control_slots: usize,
}

impl Framing {
    /// Propose a bound. The negotiated result is the smaller of the two, clamped
    /// into the accepted range, so neither side can raise the other's memory
    /// ceiling by asking.
    pub const fn new(host_max_frame_bytes: usize, extension_max_frame_bytes: usize) -> Self {
        Self {
            host_max_frame_bytes,
            extension_max_frame_bytes,
            control_slots: RESERVED_CONTROL_SLOTS,
        }
    }

    pub const fn negotiated_max_frame_bytes(self) -> usize {
        let smaller = if self.host_max_frame_bytes < self.extension_max_frame_bytes {
            self.host_max_frame_bytes
        } else {
            self.extension_max_frame_bytes
        };
        if smaller < MIN_MAX_FRAME_BYTES {
            MIN_MAX_FRAME_BYTES
        } else if smaller > MAX_MAX_FRAME_BYTES {
            MAX_MAX_FRAME_BYTES
        } else {
            smaller
        }
    }

    pub const fn control_slots(self) -> usize {
        self.control_slots
    }

    /// The bytes of queued data that may precede a control frame on a
    /// single-stream carrier.
    ///
    /// This is the honest bound to publish next to a cancellation: the request is
    /// independent of the data stream and reserved slots keep it from being
    /// starved, but a frame already being written cannot be split in the middle,
    /// so the wait is bounded rather than zero.
    pub const fn worst_case_control_wait_bytes(self) -> usize {
        self.negotiated_max_frame_bytes() * self.control_slots
    }

    /// Refuse a frame that exceeds what was negotiated.
    ///
    /// The refusal belongs to the framing interface: it names the bound, so the
    /// sender can chunk or hand over a blob handle instead, and it is never
    /// mistaken for a fault in the content the frame was carrying.
    pub fn check_frame(&self, frame_bytes: usize) -> Result<(), ApplicationFailure> {
        let bound = self.negotiated_max_frame_bytes();
        if frame_bytes <= bound {
            return Ok(());
        }
        Err(
            refusal::new("transport_frame_oversize", "extension/transport")
                .with_field("maxFrameBytes")
                .with_presentation_arg("limit", &bound.to_string()),
        )
    }
}

impl Default for Framing {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_FRAME_BYTES, DEFAULT_MAX_FRAME_BYTES)
    }
}

/// How a payload larger than one frame is delivered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OversizePolicy {
    /// Bounded chunks, each carrying its continuation in the envelope.
    Chunk,
    /// A handle the host issued for bytes it already holds or is willing to
    /// fetch under its own policy.
    BlobReference,
}

/// Whether `reference` is a controlled blob handle rather than a path an
/// extension chose.
///
/// A reference is `blob:` followed by an opaque token. Paths are refused — an
/// absolute path, a home-relative path, a Windows path, a traversal, or any
/// token containing a separator — because accepting one would turn "here is a
/// large file" into an instruction for the host to read a file the extension
/// picked. The host decides which bytes a handle resolves to, and may refuse.
pub fn blob_reference_is_controlled(reference: &str) -> bool {
    let Some(token) = reference.strip_prefix(BLOB_SCHEME) else {
        return false;
    };
    !token.is_empty()
        && token.len() <= MAX_BLOB_REFERENCE_BYTES
        && !token.starts_with(['/', '\\', '~'])
        && !token.contains("..")
        && !token.contains(['/', '\\', ':'])
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && !token.starts_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiation_takes_the_smaller_bound_and_clamps() {
        assert_eq!(
            Framing::new(128 * 1024, 64 * 1024).negotiated_max_frame_bytes(),
            64 * 1024
        );
        assert_eq!(
            Framing::new(64 * 1024, 128 * 1024).negotiated_max_frame_bytes(),
            64 * 1024
        );
        assert_eq!(
            Framing::new(1, 1).negotiated_max_frame_bytes(),
            MIN_MAX_FRAME_BYTES
        );
        assert_eq!(
            Framing::new(usize::MAX, usize::MAX).negotiated_max_frame_bytes(),
            MAX_MAX_FRAME_BYTES
        );
        assert_eq!(
            Framing::default().negotiated_max_frame_bytes(),
            DEFAULT_MAX_FRAME_BYTES
        );
    }

    #[test]
    fn control_wait_is_reported_as_a_bound() {
        let framing = Framing::new(DEFAULT_MAX_FRAME_BYTES, DEFAULT_MAX_FRAME_BYTES);
        assert_eq!(
            framing.worst_case_control_wait_bytes(),
            DEFAULT_MAX_FRAME_BYTES * RESERVED_CONTROL_SLOTS
        );
        assert!(framing.control_slots() >= 1);
    }

    #[test]
    fn oversize_frames_name_the_bound_not_the_content() {
        let framing = Framing::new(64 * 1024, 64 * 1024);
        assert!(framing.check_frame(64 * 1024).is_ok());
        let failure = framing.check_frame(64 * 1024 + 1).expect_err("over bound");
        assert_eq!(failure.code, "transport_frame_oversize");
    }

    #[test]
    fn blob_handles_are_tokens_and_paths_are_refused() {
        assert!(blob_reference_is_controlled("blob:licoup-7f3a2b"));
        assert!(blob_reference_is_controlled("blob:a.b_c-d"));
        // Not a handle at all.
        assert!(!blob_reference_is_controlled("plain-token"));
        // An absolute path, and the same path smuggled through the scheme.
        assert!(!blob_reference_is_controlled("/absolute/example"));
        assert!(!blob_reference_is_controlled("blob:/absolute/example"));
        // A home-relative path.
        assert!(!blob_reference_is_controlled("blob:~owner/example"));
        // Traversal, and a path-shaped token with separators in either direction.
        assert!(!blob_reference_is_controlled("blob:../../example"));
        assert!(!blob_reference_is_controlled("blob:parent\\child"));
        assert!(!blob_reference_is_controlled("blob:name:other"));
        assert!(!blob_reference_is_controlled("blob:"));
        assert!(!blob_reference_is_controlled("file:///absolute/example"));
    }
}
