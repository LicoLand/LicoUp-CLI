use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

/// Serializes tests that share the process-local Claude registry, including
/// tests built into separate Cargo test binaries. The directory lease is
/// process-independent; the in-process mutex avoids contending on our own PID.
pub(crate) struct ClaudeProcessLocalTestGuard {
    _local: MutexGuard<'static, ()>,
    lease_directory: PathBuf,
}

pub(crate) fn lock_claude_process_local_tests() -> ClaudeProcessLocalTestGuard {
    static LOCAL: OnceLock<Mutex<()>> = OnceLock::new();
    let local = LOCAL
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let lease_directory = std::env::temp_dir().join("lico-claude-process-local-test.lock");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match fs::create_dir(&lease_directory) {
            Ok(()) => {
                fs::write(
                    lease_directory.join("owner"),
                    std::process::id().to_string(),
                )
                .expect("write process-local test lease owner");
                return ClaudeProcessLocalTestGuard {
                    _local: local,
                    lease_directory,
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if stale_lease(&lease_directory) {
                    let _ = fs::remove_dir_all(&lease_directory);
                    continue;
                }
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for Claude process-local test lease"
                );
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("create process-local test lease: {error}"),
        }
    }
}

fn stale_lease(lease_directory: &std::path::Path) -> bool {
    let Ok(owner) = fs::read_to_string(lease_directory.join("owner")) else {
        return false;
    };
    let Ok(pid) = owner.trim().parse::<u32>() else {
        return true;
    };
    !Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

impl Drop for ClaudeProcessLocalTestGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.lease_directory.join("owner"));
        let _ = fs::remove_dir(&self.lease_directory);
    }
}
