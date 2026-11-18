//! Compiles the ACP session-protocol state machines this crate owns.
//!
//! The two machines' transition tables live here because the transport engines
//! that drive them — `acp_driver_runtime::protocol` and
//! `acp_session_transport::protocol` — live here; a machine whose configuration
//! stayed behind with code that did not would generate nothing and fail to
//! resolve.

fn main() {
    println!("cargo:rerun-if-changed=resources/state-machines");
    licoup_state_machine_codegen::generate_directory("resources/state-machines")
        .expect("agent-drivers state-machine configuration must compile");
}
