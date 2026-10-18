// The packaged generic-CLI registration document and its reader moved to
// `licoup-agent-targets` with the Agent declarations they belong to. The former
// path stays reachable through this re-export for the driver dispatch, the
// generic CLI driver and the Agent catalog that still read it here.
pub use licoup_agent_targets::domain::cli_registration::*;
