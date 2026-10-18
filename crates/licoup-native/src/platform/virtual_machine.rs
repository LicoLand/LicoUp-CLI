// The SSH virtual-machine target contract moved to `licoup-agent-targets` with
// the rest of the Agent inventory. The former path stays reachable at the
// visibility this crate exposed before the move, so the driver dispatch and the
// ACP session transport that still live here keep compiling.
pub(crate) use licoup_agent_targets::platform::virtual_machine::*;
