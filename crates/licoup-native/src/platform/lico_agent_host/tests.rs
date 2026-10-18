//! The Plan profile the package owns, run under the client's real sandbox.
//!
//! The profile's semantics cannot be proved by reading its text: whether the one
//! literal plan file is writable and whether a sibling path is not are questions
//! only the platform's sandbox can answer. These are host-owned integration
//! claims, so they live here and drive the package's own
//! `licoup_agent_lico_agent::driver::plan_command` through this client's answer
//! for its sandbox port.

use licoup_agent_lico_agent::driver::plan_command;

use crate::platform::lico_agent_host;
use crate::platform::process_sandbox::sandbox_exec_can_apply;

fn install_sandbox_port() {
    // Installation is first-wins per process, so a repeat in this test binary is
    // refused rather than fatal.
    let _ = licoup_agent_lico_agent::port::sandbox::install(lico_agent_host::sandbox_port());
}

#[test]
fn plan_profile_allows_literal_plan_write() {
    use std::fs;
    use uuid::Uuid;
    if !sandbox_exec_can_apply() {
        return;
    }
    install_sandbox_port();
    let root = std::env::temp_dir().join(format!("licoup-sb-plan-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let plan = root.join("active-plan.md");
    fs::write(&plan, b"").unwrap();
    let workspace = root.join("workspace");
    fs::create_dir(&workspace).unwrap();
    // Prefer a single literal binary over /bin/sh: seatbelt process-exec of
    // /bin/sh can fail when the host needs to resolve shell variants.
    let mut command = plan_command(
        std::path::Path::new("/usr/bin/tee"),
        &plan,
        &workspace,
        15_722,
        &[plan.display().to_string()],
    )
    .unwrap();
    command.stdin(std::process::Stdio::piped());
    let mut child = command.spawn().unwrap();
    use std::io::Write;
    child.stdin.as_mut().unwrap().write_all(b"ok").unwrap();
    let status = child.wait().unwrap();
    assert!(status.success());
    assert_eq!(fs::read_to_string(&plan).unwrap(), "ok");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn plan_profile_denies_sibling_write() {
    use std::fs;
    use uuid::Uuid;
    if !sandbox_exec_can_apply() {
        return;
    }
    install_sandbox_port();
    let root = std::env::temp_dir().join(format!("licoup-sb-deny-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let plan = root.join("active-plan.md");
    fs::write(&plan, b"").unwrap();
    let sibling = root.join("other.md");
    let workspace = root.join("workspace");
    fs::create_dir(&workspace).unwrap();
    let mut command = plan_command(
        std::path::Path::new("/usr/bin/tee"),
        &plan,
        &workspace,
        15_722,
        &[sibling.display().to_string()],
    )
    .unwrap();
    command.stdin(std::process::Stdio::piped());
    let mut child = command.spawn().unwrap();
    use std::io::Write;
    let _ = child.stdin.as_mut().unwrap().write_all(b"x");
    let status = child.wait().unwrap();
    assert!(!status.success());
    assert!(!sibling.exists() || fs::read_to_string(&sibling).unwrap_or_default() != "x");
    let _ = fs::remove_dir_all(root);
}
