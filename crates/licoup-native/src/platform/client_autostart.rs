// The client, MCP and Gateway login autostart entries moved to
// `licoup-agent-targets` with the target declarations they serve. The former
// path stays reachable through this re-export for the FFI autostart command
// layer, which still lives here. The local model gateway's own autostart state
// and its post-recovery rewrite arrive through the inventory port that this
// host composes.
pub use licoup_agent_targets::platform::client_autostart::*;
