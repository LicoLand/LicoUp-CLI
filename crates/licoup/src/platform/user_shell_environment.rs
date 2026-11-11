// The user login-shell environment snapshot moved to `licoup-agent-targets`
// with the Agent declarations whose launches observe it. The former path stays
// reachable for the driver engines and the extension managers that still start
// Agent CLI children from here.
pub use licoup_agent_targets::platform::user_shell_environment::*;
