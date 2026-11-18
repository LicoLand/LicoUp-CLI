/// The runtime protocol this package's driver executes: one app-server process
/// framed over stdio as JSON-RPC, one line per message.
pub const RUNTIME_PROTOCOL: &str = "codex-app-server-stdio-jsonrpc";

/// The published format identity of the wire this package's entry reads.
///
/// The release declaration names it as the converter's source format, so the
/// declaration and the protocol the binary actually speaks are one fact rather
/// than two strings that can drift.
pub const PROTOCOL_FORMAT: &str = "codex.app-server.v1";
