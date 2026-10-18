fn main() {
    let source = "resources/state-machines.json";
    println!("cargo:rerun-if-changed={source}");
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"))
        .join("state_machines.rs");
    licoup_state_machine_codegen::compile_file(source, output)
        .expect("compile conversation state machines");
}
