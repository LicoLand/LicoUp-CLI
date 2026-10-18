//! Local lifecycle control for this executable, independent of the kernel.
use crate::{
    private_state,
    transport::{self, SubagentMcpSupervisor},
};
use anyhow::{Result, anyhow};
use fs2::FileExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

/// The digest the launcher approved for the bytes it started.
///
/// The native package lifecycle measures the installed generation's executable
/// and hands that digest to the program it starts. A serving process that finds
/// a different digest on itself refuses to serve: the payload that comes up is
/// the byte-identical one the operator approved, or it is nothing.
const APPROVED_DIGEST_ENV: &str = "LICOUP_MCP_APPROVED_DIGEST";

/// The largest executable this process will read to measure itself.
const MAX_PAYLOAD_BYTES: u64 = 512 * 1024 * 1024;

fn digest_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).map_err(|_| anyhow!("mcp_binary_unavailable"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut read = 0_u64;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| anyhow!("mcp_binary_unavailable"))?;
        if count == 0 {
            break;
        }
        read += count as u64;
        if read > MAX_PAYLOAD_BYTES {
            return Err(anyhow!("mcp_binary_unavailable"));
        }
        hasher.update(&buffer[..count]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    Ok(format!("sha256:{hex}"))
}

/// Whether a measured payload satisfies the approval it was started under.
///
/// No approval is not a mismatch: an executable a developer runs directly is
/// answerable to that developer, and the launcher that binds consent always
/// sends one.
fn consent_holds(measured: &str, approved: Option<&str>) -> Result<()> {
    match approved {
        Some(approved) if approved != measured => Err(anyhow!("mcp_payload_not_approved")),
        _ => Ok(()),
    }
}

fn check_approved_payload() -> Result<()> {
    let approved = env::var(APPROVED_DIGEST_ENV).ok();
    let approved = approved.as_deref().filter(|value| !value.is_empty());
    if approved.is_none() {
        return Ok(());
    }
    let executable = env::current_exe().map_err(|_| anyhow!("mcp_binary_unavailable"))?;
    let measured = digest_file(&executable)?;
    consent_holds(&measured, approved)
}

fn current() -> Option<transport::DiscoveryDocument> {
    transport::discovery_path_read_only()
        .ok()
        .and_then(|path| transport::read_discovery(&path).ok())
}
fn status() -> Value {
    let running = current().is_some_and(|document| {
        let Some((caller, _)) = document.tokens.iter().next() else {
            return false;
        };
        transport::load_connector_discovery(caller)
            .ok()
            .is_some_and(|discovery| transport::connector_health(&discovery))
    });
    json!({"service":"subagents","state":if running {"running"} else {"stopped"}})
}
fn service_lease_held() -> bool {
    let Ok(root) = private_state::portable_data_dir_read_only() else {
        return false;
    };
    let Ok(lease) = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("client-state/subagent-mcp/service.lock"))
    else {
        return false;
    };
    match lease.try_lock_exclusive() {
        Ok(()) => false,
        Err(error) => error.kind() == std::io::ErrorKind::WouldBlock,
    }
}
fn clean_stale_generation(generation: &str) {
    // Hold the OS lease while inspecting/removing a crashed generation, so a
    // newly starting service cannot publish between that check and deletion.
    let Ok(root) = private_state::portable_data_dir_read_only() else {
        return;
    };
    let Ok(lease) = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("client-state/subagent-mcp/service.lock"))
    else {
        return;
    };
    if lease.try_lock_exclusive().is_err() {
        return;
    }
    if current().is_some_and(|value| value.generation == generation) {
        if let Ok(path) = transport::discovery_path_read_only() {
            let _ = fs::remove_file(path);
        }
    }
}
fn start() -> Result<Value> {
    if status()["state"] == "running" {
        return Ok(status());
    }
    let executable = env::current_exe().map_err(|_| anyhow!("mcp_binary_unavailable"))?;
    let mut child = Command::new(executable)
        .args(["service", "serve"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| anyhow!("mcp_start_failed"))?;
    loop {
        if status()["state"] == "running" {
            return Ok(status());
        }
        if child.try_wait()?.is_some() {
            // A simultaneous starter can win the exclusive service lease.
            let state = status();
            if state["state"] == "running" {
                return Ok(state);
            }
            if !service_lease_held() {
                return Err(anyhow!("mcp_start_failed"));
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
}
fn stop() -> Result<Value> {
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(2))
        .build();
    let document = loop {
        let Some(document) = current() else {
            if !service_lease_held() {
                return Ok(status());
            }
            // A service may hold its process lease while its listener is
            // starting and before it publishes discovery. Keep waiting for
            // the owned generation to become addressable or to exit.
            thread::sleep(Duration::from_millis(25));
            continue;
        };
        let endpoint = document
            .endpoint
            .strip_suffix("/mcp")
            .ok_or_else(|| anyhow!("mcp_state_invalid"))?;
        match agent
            .post(&format!("{endpoint}/control/stop"))
            .set(
                "authorization",
                &format!("Bearer {}", document.control_token),
            )
            .send_bytes(&[])
        {
            Ok(response) if response.status() == 202 => break document,
            Err(ureq::Error::Transport(_)) if !service_lease_held() => {
                clean_stale_generation(&document.generation);
                return Ok(status());
            }
            Err(ureq::Error::Transport(_)) => {
                // A transient connect failure is not evidence that the
                // process released its data-root lease. Retry until its
                // lifecycle lock proves exit, or the stop endpoint responds.
                thread::sleep(Duration::from_millis(25));
            }
            _ => return Err(anyhow!("mcp_stop_failed")),
        }
    };
    // Drain acknowledged protocol calls, without an arbitrary task deadline.
    while current().is_some_and(|value| value.generation == document.generation) {
        if !service_lease_held() {
            clean_stale_generation(&document.generation);
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    Ok(json!({"service":"subagents","state":"stopped"}))
}
fn serve() -> Result<Value> {
    // Consent before serving: the process that answers callers is the approved
    // payload, or it does not come up at all.
    check_approved_payload()?;
    let root = private_state::portable_data_dir()?.join("client-state/subagent-mcp");
    private_state::ensure_private_dir(&root)?;
    let mut options = fs::OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lease = options.open(root.join("service.lock"))?;
    lease
        .try_lock_exclusive()
        .map_err(|_| anyhow!("mcp_service_already_running"))?;
    // Catalog startup is read-only. The native process performs all commands.
    let service = SubagentMcpSupervisor::start()?;
    while !service.stopped() && service.healthy() {
        thread::sleep(Duration::from_millis(50));
    }
    drop(service);
    drop(lease);
    Ok(json!({"service":"subagents","state":"stopped"}))
}
pub fn execute(action: &str) -> Result<Value> {
    match action {
        "start" => start(),
        "stop" => stop(),
        "status" => Ok(status()),
        "reload" => {
            stop()?;
            start()
        }
        "serve" => serve(),
        _ => Err(anyhow!("mcp_lifecycle_invalid")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(tag: &str, content: &[u8]) -> (std::path::PathBuf, std::path::PathBuf) {
        let root = env::temp_dir().join(format!(
            "lico-mcp-lifecycle-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&root).expect("fixture root");
        let payload = root.join("lico-subagent-mcp");
        fs::write(&payload, content).expect("fixture payload");
        (root, payload)
    }

    #[test]
    fn only_the_approved_bytes_may_serve() {
        let (root, payload) = fixture("consent", b"the approved payload");
        let approved = digest_file(&payload).expect("the fixture measures");
        assert!(approved.starts_with("sha256:"));
        assert!(
            consent_holds(&approved, Some(approved.as_str())).is_ok(),
            "the measured bytes are the approved bytes"
        );
        assert!(
            consent_holds(&approved, None).is_ok(),
            "an unbound direct run is answerable to the developer who started it"
        );

        let (other_root, other) = fixture("swap", b"some other payload");
        let swapped = digest_file(&other).expect("the other fixture measures");
        assert_ne!(approved, swapped);
        assert_eq!(
            consent_holds(&swapped, Some(approved.as_str()))
                .expect_err("a swap is refused")
                .to_string(),
            "mcp_payload_not_approved"
        );

        fs::remove_dir_all(root).expect("cleanup");
        fs::remove_dir_all(other_root).expect("cleanup");
    }

    #[test]
    fn a_digest_is_the_bytes_and_never_the_path() {
        let (root, payload) = fixture("same-bytes", b"identical");
        let (other_root, other) = fixture("same-bytes-again", b"identical");
        assert_eq!(
            digest_file(&payload).expect("measure"),
            digest_file(&other).expect("measure"),
            "the same bytes measure the same wherever they are"
        );
        fs::remove_dir_all(root).expect("cleanup");
        fs::remove_dir_all(other_root).expect("cleanup");
    }
}
