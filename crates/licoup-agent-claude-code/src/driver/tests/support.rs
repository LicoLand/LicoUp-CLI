use super::*;

pub(super) fn absolute_test_cwd() -> PathBuf {
    std::env::current_dir().expect("test working directory")
}

pub(super) fn config(params: Value, prompt: &str, session_id: &str) -> DriverConfig {
    DriverConfig::from_params(
        &params,
        prompt,
        session_id,
        Some(absolute_test_cwd().as_path()),
    )
    .unwrap()
}

pub(super) fn temporary_directory(prefix: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{nonce}"));
    fs::create_dir_all(&path).unwrap();
    path
}

pub(super) fn compile_fake_claude(prefix: &str) -> (PathBuf, PathBuf) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_claude_code.rs");
    let directory = temporary_directory(prefix);
    let executable = directory.join(format!("fake-claude{}", std::env::consts::EXE_SUFFIX));
    let status = Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string()))
        .arg("--edition=2024")
        .arg(&fixture)
        .arg("-o")
        .arg(&executable)
        .status()
        .unwrap();
    assert!(status.success());
    (directory, executable)
}

pub(super) fn process_local_test_guard()
-> super::claude_process_local_test_lock::ClaudeProcessLocalTestGuard {
    super::claude_process_local_test_lock::lock_claude_process_local_tests()
}

#[cfg(unix)]
pub(super) fn wait_for_descendant_pid(directory: &Path) -> u32 {
    let path = directory.join("fake-claude-descendant.pid");
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(value) = fs::read_to_string(&path)
            && let Ok(pid) = value.trim().parse::<u32>()
        {
            return pid;
        }
        assert!(
            Instant::now() < deadline,
            "descendant pid was not published"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
pub(super) fn process_exists(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
