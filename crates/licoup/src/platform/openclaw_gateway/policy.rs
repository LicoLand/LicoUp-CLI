/// The endpoint policy OpenClaw owns.
///
/// The vendor-default port, the port this client prefers and the attach-mode
/// vocabulary are facts about OpenClaw rather than about this client, so they
/// live in the adapter package that carries the Agent
/// ([`licoup_agent_openclaw::policy`]) and are read here rather than restated.
/// What stays in this module is the engine's own operational configuration: the
/// state directory and schema, the preferred-port scan span, the reserved ports
/// and the stable failure codes.
pub(super) use licoup_agent_openclaw::policy::{DEFAULT_PORT, VENDOR_DEFAULT_PORT};

pub(super) const PORT_RANGE_SPAN: u16 = 16;
pub(super) const DEFAULT_HOST: &str = "127.0.0.1";
pub(super) const STATE_DIR: &str = "openclaw-gateway";
pub(super) const DEFAULT_HEALTH_TIMEOUT_MS: u64 = 60_000;
pub(super) const STATE_SCHEMA_VERSION: &str = "v0.0.1:openclaw-gateway-2";
pub(super) const INVALID_STATE: &str = "openclaw_gateway_state_invalid";
pub(super) const EXECUTABLE_MISSING: &str = "openclaw_executable_missing";
pub(super) const PORT_EXHAUSTED: &str = "openclaw_gateway_port_exhausted";
pub(super) const START_FAILED: &str = "openclaw_gateway_start_failed";
pub(super) const HEALTH_FAILED: &str = "openclaw_gateway_health_failed";
pub(super) const STOP_FAILED: &str = "openclaw_gateway_stop_failed";

pub(super) const RESERVED_PORTS: &[u16] = &[
    3000, 4096, 5173, 7228, 8080, 8443, 17328, 17329, 18765, 18789, 19001, 24173,
];
