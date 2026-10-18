//! Same-executable controls: exec permission is identical in both modes.
//! The native helper contains the strictly bounded vfork child; no unsafe Rust
//! exemption or production process owner is added.

#[cfg(target_os = "macos")]
#[test]
fn reference_interpreter_spawns_the_same_allowed_target_only_in_control() {
    use crate::support::*;
    use licoup_native::platform::extension_host::ExtensionCarrier;
    use licoup_native::platform::extension_host::isolation::IsolationMode;
    use std::time::Duration;
    let python = python_runtime();
    let sandbox = Sandbox::new("python-creation");
    let script = sandbox.fixture_dir().join("python_creation.py");
    std::fs::write(&script, include_str!("fixtures/python_creation.py")).expect("fixture");
    for route in ["os.posix_spawn", "subprocess"] {
        for mode in [IsolationMode::TrustedLocal, IsolationMode::Restricted] {
            let identity = format!("{}-{route}", mode.id());
            let instance = sandbox.instance_root(&identity);
            let program = python_program(
                &python,
                vec![
                    "-B".into(),
                    script.display().to_string(),
                    python.executable.display().to_string(),
                    route.into(),
                ],
                &sandbox,
                &instance,
            )
            .with_exec_path(&python.executable);
            let carrier = carrier(
                programs("acme.python.creation/ext", "1.0.0", program),
                policy(mode, sandbox.root()),
            );
            let session = carrier
                .start(&carrier_spec(
                    "acme.python.creation/ext",
                    "1.0.0",
                    &identity,
                    1,
                ))
                .expect("start");
            assert!(
                wait_for(Duration::from_secs(14), || instance
                    .join("python-creation.json")
                    .is_file()),
                "runtime probe must finish"
            );
            let result: serde_json::Value = serde_json::from_str(
                &read_text(&instance.join("python-creation.json")).expect("outcome"),
            )
            .expect("JSON");
            let released = carrier.shutdown(&session);
            assert_eq!(result["route"], route);
            match mode {
                IsolationMode::TrustedLocal => {
                    assert_eq!(result["outcome"], "created");
                    assert_eq!(result["exit"], 0);
                    assert_eq!(
                        read_text(&instance.join("python-child.executed")).as_deref(),
                        Some("allowed-target")
                    );
                    assert_eq!(
                        released.expect_err("scoped release").code,
                        "extension_isolation_descendants_unverified"
                    );
                }
                IsolationMode::Restricted => {
                    assert_eq!(result["outcome"], "denied");
                    assert_eq!(result["errno"], 1);
                    assert!(!instance.join("python-child.executed").exists());
                    released.expect("single process release");
                }
            }
            eprintln!("X2_PROBE interpreter {} {}", mode.id(), result);
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn same_allowed_target_creation_controls() {
    use crate::support::{Sandbox, carrier, carrier_spec, policy, programs, read_text, wait_for};
    use licoup_native::platform::extension_host::ExtensionCarrier;
    use licoup_native::platform::extension_host::isolation::{
        IsolationMode, ReleaseScope, ResolvedProgram,
    };
    use std::process::Command;
    use std::time::Duration;

    let sandbox = Sandbox::new("native-creation");
    let source = sandbox.fixture_dir().join("process_creation.c");
    let executable = sandbox.fixture_dir().join("process-creation");
    std::fs::write(&source, include_str!("fixtures/process_creation.c"))
        .expect("synthetic C source");
    let compile = Command::new("/usr/bin/cc")
        .args([
            "-std=c11",
            "-D_DARWIN_C_SOURCE",
            "-O0",
            "-Wall",
            "-Wextra",
            "-Werror",
        ])
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("UNVERIFIABLE: native compiler unavailable; no OS evidence was produced");
    assert!(
        compile.status.success(),
        "UNVERIFIABLE: native probe did not compile"
    );
    let executable = executable.canonicalize().expect("canonical executable");
    use sha2::Digest;
    let hash = sha2::Sha256::digest(std::fs::read(&executable).expect("native executable bytes"));
    eprintln!("X2_RUNTIME native-probe-executable-sha256={hash:x}");
    for route in ["fork-setsid", "fork-setpgid", "posix_spawn", "vfork-exec"] {
        for mode in [IsolationMode::TrustedLocal, IsolationMode::Restricted] {
            let identity = format!("{}-{route}", mode.id());
            let instance = sandbox.instance_root(&identity);
            let program = ResolvedProgram::new(&executable)
                .with_args([route])
                .with_exec_path(&executable)
                .with_read_root(sandbox.fixture_dir())
                .with_write_root(&instance);
            let carrier = carrier(
                programs("acme.creation/ext", "1.0.0", program),
                policy(mode, sandbox.root()),
            );
            let session = carrier
                .start(&carrier_spec("acme.creation/ext", "1.0.0", &identity, 1))
                .expect("start same allowed target");
            assert!(
                wait_for(Duration::from_secs(12), || read_text(
                    &instance.join("creation.outcome")
                )
                .is_some_and(|s| s.ends_with('\n'))),
                "probe did not finish {route}"
            );
            assert_eq!(
                read_text(&instance.join("parent.started")).as_deref(),
                Some("initial-exec-allowed\n")
            );
            let outcome = read_text(&instance.join("creation.outcome")).expect("outcome");
            let shutdown = carrier.shutdown(&session);
            match mode {
                IsolationMode::TrustedLocal => {
                    assert_eq!(outcome, format!("{route} created 0 0\n"));
                    assert_eq!(
                        read_text(&instance.join("child.executed")).as_deref(),
                        Some("same-allowed-executable\n")
                    );
                    assert_eq!(
                        shutdown.expect_err("trusted release stays scoped").code,
                        "extension_isolation_descendants_unverified"
                    );
                }
                IsolationMode::Restricted => {
                    assert_eq!(
                        outcome,
                        format!("{route} denied 1 -1\n"),
                        "same allowed target must not create a descendant"
                    );
                    assert!(!instance.join("child.executed").exists());
                    shutdown.expect("single process exit confirmed");
                    assert_eq!(
                        carrier.facts(&identity).expect("facts").release_scope(),
                        Some(ReleaseScope::Instance)
                    );
                }
            }
            eprintln!("X2_PROBE creation {} {}", mode.id(), outcome.trim());
        }
    }
}
