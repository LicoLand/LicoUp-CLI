//! The Gateway endpoint policy this Agent owns.
//!
//! OpenClaw serves one Gateway over a loopback endpoint pair, and *which* port
//! its own program listens on by default, *which* port this client prefers for a
//! Gateway it owns, and *how* an attach names the mode it found are facts about
//! OpenClaw rather than about LicoUp. They live here, in the package that
//! carries the Agent, as data; the client's Gateway lifecycle reads them and
//! supplies the two things it owns — the port scan, the state document, the
//! process it starts and stops, and the bounded HTTP probe that decides health.
//!
//! Nothing here is a decision the package enforces: the package names the
//! endpoints and the vocabulary, and the engine that binds sockets, writes state
//! and stops processes is reached through [`crate::port::gateway`].

/// The port OpenClaw's own Gateway listens on when the vendor program starts it,
/// and therefore the endpoint an attach reuses rather than one this client
/// started.
pub const VENDOR_DEFAULT_PORT: u16 = 18_789;

/// The port this client prefers for a Gateway it owns.
///
/// It is deliberately not the vendor default: an owned Gateway must never take
/// the port the vendor program would own, or a later attach could not tell the
/// two apart.
pub const DEFAULT_PORT: u16 = 24_189;

/// How an attach names the endpoint mode it found.
///
/// The vocabulary is OpenClaw's: an attach that reached the vendor program's own
/// default port is a *vendor-default* attach, and one that reached a Gateway
/// this client started or reused on any other port is owned or shared. The
/// client's status documents and the driver's `dispatch.gateway.attached` event
/// both report this answer rather than each wording their own.
pub fn attach_mode(port: u16) -> &'static str {
    if port == VENDOR_DEFAULT_PORT {
        "vendor-default"
    } else {
        "managed-or-reused"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vendor_default_port_is_not_the_port_this_client_prefers() {
        assert_eq!(VENDOR_DEFAULT_PORT, 18_789);
        assert_eq!(DEFAULT_PORT, 24_189);
        assert_ne!(VENDOR_DEFAULT_PORT, DEFAULT_PORT);
    }

    #[test]
    fn only_the_vendor_default_port_is_named_a_vendor_default_attach() {
        assert_eq!(attach_mode(VENDOR_DEFAULT_PORT), "vendor-default");
        assert_eq!(attach_mode(DEFAULT_PORT), "managed-or-reused");
    }
}
