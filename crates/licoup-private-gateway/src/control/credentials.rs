//! Private Unix control channel for hot-applying Gateway credentials.
//!
//! The sidecar binds a mode-0600 socket under the LLM Gateway state directory.
//! The host CLI (same uid) pushes a handoff or clear message after authorize /
//! clear. Secrets never land on disk; only the live socket carries them.

#[cfg(unix)]
use crate::credential::{GatewayCredentialHandoff, GatewayCredentialSlot, LlmApiKeyProvider};
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

const MAX_CONTROL_BYTES: usize = 4 * 1024 * 1024;
const CONTROL_SOCKET_NAME: &str = "credentials.sock";

pub fn control_socket_path(state_root: &Path) -> PathBuf {
    state_root.join(CONTROL_SOCKET_NAME)
}

/// Serve credential hot-apply requests until `stop` is set.
#[cfg(unix)]
pub fn serve_credentials_control(
    socket_path: PathBuf,
    credentials: Arc<GatewayCredentialSlot>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    use std::os::unix::net::UnixListener;

    let _ = std::fs::remove_file(&socket_path);
    if let Some(parent) = socket_path.parent() {
        licoup_foundation::platform::file_security::ensure_private_dir(parent)?;
    }
    let listener = UnixListener::bind(&socket_path)
        .map_err(|error| anyhow!("llm_gateway_credentials_control_bind_failed:{error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600));
    }
    listener
        .set_nonblocking(true)
        .map_err(|_| anyhow!("llm_gateway_credentials_control_bind_failed"))?;

    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                if !peer_uid_allowed(&stream) {
                    let _ = write_framed(
                        &mut stream,
                        &json!({"ok": false, "error": "llm_gateway_credentials_control_denied"}),
                    );
                    continue;
                }
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                let response = match handle_control_message(&mut stream, &credentials) {
                    Ok(loaded) => json!({
                        "ok": true,
                        "credentialsLoaded": loaded,
                    }),
                    Err(error) => json!({
                        "ok": false,
                        "error": error.to_string(),
                    }),
                };
                let _ = write_framed(&mut stream, &response);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(_) => break,
        }
    }
    let _ = std::fs::remove_file(&socket_path);
    Ok(())
}

#[cfg(not(unix))]
pub fn serve_credentials_control(
    _socket_path: PathBuf,
    _credentials: Arc<GatewayCredentialSlot>,
    _stop: Arc<AtomicBool>,
) -> Result<()> {
    Err(anyhow!("llm_gateway_credentials_control_unsupported"))
}

/// Push the current session handoff (or clear) to a running sidecar.
#[cfg(unix)]
pub fn apply_credentials_hot(
    socket_path: &Path,
    handoff: Option<&GatewayCredentialHandoff>,
) -> Result<bool> {
    use std::os::unix::net::UnixStream;

    ensure!(
        socket_path.is_absolute(),
        "llm_gateway_credentials_control_unavailable"
    );
    let mut stream = UnixStream::connect(socket_path)
        .map_err(|_| anyhow!("llm_gateway_credentials_control_unavailable"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let payload = handoff.map(|handoff| handoff.to_json()).transpose()?;
    write_framed_bytes(
        &mut stream,
        payload
            .as_ref()
            .map_or(&[][..], |payload| payload.expose_bytes()),
    )?;
    let response = read_framed_json(&mut stream)?;
    if response.get("ok").and_then(Value::as_bool) != Some(true) {
        let code = response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("llm_gateway_credentials_control_failed")
            .to_owned();
        return Err(anyhow!(code));
    }
    Ok(response
        .get("credentialsLoaded")
        .and_then(Value::as_bool)
        .unwrap_or(false))
}

#[cfg(not(unix))]
pub fn apply_credentials_hot(_socket_path: &Path, _handoff: Option<&()>) -> Result<bool> {
    Err(anyhow!("llm_gateway_credentials_control_unsupported"))
}

#[cfg(unix)]
fn handle_control_message(
    stream: &mut impl Read,
    credentials: &GatewayCredentialSlot,
) -> Result<bool> {
    let payload = read_framed_bytes(stream)?;
    if payload.is_empty() {
        credentials.clear()?;
        return Ok(false);
    }
    let handoff = GatewayCredentialHandoff::from_json(&payload)?;
    let lease =
        credential_lease::port::require::<LlmApiKeyProvider>()?.lease_from_handoff(handoff)?;
    credentials.replace(lease)?;
    Ok(credentials.connected())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn peer_uid_allowed(stream: &std::os::unix::net::UnixStream) -> bool {
    use std::os::fd::AsRawFd;
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    rc == 0 && credentials.uid == unsafe { libc::getuid() }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn peer_uid_allowed(stream: &std::os::unix::net::UnixStream) -> bool {
    use std::os::fd::AsRawFd;
    let mut uid = 0u32;
    let mut gid = 0u32;
    let rc = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    rc == 0 && uid == unsafe { libc::getuid() }
}

fn write_framed(stream: &mut impl Write, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    write_framed_bytes(stream, &bytes)
}

fn write_framed_bytes(stream: &mut impl Write, payload: &[u8]) -> Result<()> {
    ensure!(
        payload.len() <= MAX_CONTROL_BYTES,
        "llm_gateway_credentials_control_message_invalid"
    );
    let len = u32::try_from(payload.len())
        .map_err(|_| anyhow!("llm_gateway_credentials_control_message_invalid"))?;
    stream.write_all(&len.to_le_bytes())?;
    stream.write_all(payload)?;
    stream.flush()?;
    Ok(())
}

fn read_framed_bytes(stream: &mut impl Read) -> Result<Vec<u8>> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header)?;
    let len = u32::from_le_bytes(header) as usize;
    ensure!(
        len <= MAX_CONTROL_BYTES,
        "llm_gateway_credentials_control_message_invalid"
    );
    let mut payload = vec![0u8; len];
    if len > 0 {
        stream.read_exact(&mut payload)?;
    }
    Ok(payload)
}

fn read_framed_json(stream: &mut impl Read) -> Result<Value> {
    let payload = read_framed_bytes(stream)?;
    serde_json::from_slice(&payload)
        .map_err(|_| anyhow!("llm_gateway_credentials_control_message_invalid"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn control_socket_clear_message_disconnects_a_live_slot() {
        use std::os::unix::fs::PermissionsExt;
        // macOS sockaddr_un path length is tight; keep the fixture path short.
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lgw-{stamp}"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = control_socket_path(&root);
        let credentials = Arc::new(GatewayCredentialSlot::disconnected());
        let stop = Arc::new(AtomicBool::new(false));
        let server_credentials = Arc::clone(&credentials);
        let server_stop = Arc::clone(&stop);
        let server_socket = socket.clone();
        let server = thread::spawn(move || {
            serve_credentials_control(server_socket, server_credentials, server_stop).unwrap();
        });
        for _ in 0..40 {
            if socket.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        assert!(socket.exists());
        let loaded = apply_credentials_hot(&socket, None).unwrap();
        assert!(!loaded);
        assert!(!credentials.connected());
        stop.store(true, Ordering::SeqCst);
        let _ = server.join();
        let _ = std::fs::remove_dir_all(&root);
    }
}
