//! Compiles the state machines this crate owns. The native-parser lifecycle
//! machine's transition table lives here because `lifecycle.rs`, which drives
//! it, lives here; a machine whose configuration stayed behind with code that
//! did not would generate nothing and fail to resolve.

fn main() {
    println!("cargo:rerun-if-changed=resources/state-machines");
    licoup_state_machine_codegen::generate_directory("resources/state-machines")
        .expect("adapter SDK state-machine configuration must compile");
}
