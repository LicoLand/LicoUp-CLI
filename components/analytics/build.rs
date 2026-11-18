//! Compiles this package's declarative state machine into the module
//! `src/lib.rs` includes.
//!
//! The JSON configuration is the transition authority: `state_machine::analytics_package`
//! is generated from `resources/state-machines/package.json` at build time, so
//! the activation and uninstall order the package follows cannot drift from the
//! declaration the package publishes.

fn main() {
    licoup_state_machine_codegen::generate_directory("resources/state-machines")
        .expect("compile analytics state machines");
}
