mod adapter_dispatch;
mod agent_execution_port;
mod approval_authority;
mod artifact;
mod claude_code_package;
mod conversation_integrity;
mod generic_cli;
mod normalization;
mod pi_turn_events;
mod probe;
mod protocol_selector;
mod registry;

/// Compose this host for the suites below.
///
/// The moved crate reads every Agent through the port this module installs, so
/// a suite that calls a registry or dispatch entry directly has to compose the
/// host first — which the production entry points do for themselves.
pub(super) fn compose() {
    super::install();
}
