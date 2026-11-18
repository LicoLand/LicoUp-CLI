//! Conversation lane port — the `a4mc` contract surface.
//!
//! The Telegram channel answers turns by asking the composing host for lane
//! work. The runtime never links conversation or host internals; the host
//! registers its four answers once per process, and without them every lane
//! call fails closed. The port itself is defined by the `a4mc` contract and
//! re-exported here so the channel stack and the host's composition meet on
//! one type — never two copies drifting apart across the seam.
//!
//! `ChannelError` is the error type the installed answers return.

pub use a4mc::ChannelError;
pub use a4mc::lane::*;
