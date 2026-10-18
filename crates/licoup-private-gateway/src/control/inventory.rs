//! Private Unix control channel for partial hot-reload of conversation readiness.
//!
//! When verified readiness changes, the host pushes the readiness document to
//! the running gateway sidecar. Telegram `/agent` admits newly ready agents
//! without restarting the process and without clearing bindings or in-use
//! conversation sessions. An on-disk overlay keeps the next boot consistent
//! with the last hot-applied document when live apply is unavailable.

use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

const MAX_CONTROL_BYTES: usize = 4 * 1024 * 1024;
const CONTROL_SOCKET_NAME: &str = "inventory.sock";
pub const OVERLAY_FILE_NAME: &str = "conversation-readiness.overlay.json";

pub fn control_socket_path(state_root: &Path) -> PathBuf {
    state_root.join(CONTROL_SOCKET_NAME)
}

pub fn overlay_path(state_root: &Path) -> PathBuf {
    state_root.join(OVERLAY_FILE_NAME)
}

/// Serve readiness hot-reload requests until `stop` is set.
#[cfg(unix)]
pub fn serve_inventory_control(socket_path: PathBuf, stop: Arc<AtomicBool>) -> Result<()> {
    use std::os::unix::net::UnixListener;

    let _ = std::fs::remove_file(&socket_path);
    if let Some(parent) = socket_path.parent() {
        licoup_foundation::platform::file_security::ensure_private_dir(parent)?;
    }
    let listener = UnixListener::bind(&socket_path)
        .map_err(|error| anyhow!("gateway_inventory_control_bind_failed:{error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600));
    }
    listener
        .set_nonblocking(true)
        .map_err(|_| anyhow!("gateway_inventory_control_bind_failed"))?;

    let overlay = overlay_path(
        socket_path
            .parent()
            .ok_or_else(|| anyhow!("gateway_inventory_control_bind_failed"))?,
    );

    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                // Accepted peers inherit the listener's nonblocking flag on some
                // platforms; force blocking so framed readiness payloads can complete.
                let _ = stream.set_nonblocking(false);
                if !peer_uid_allowed(&stream) {
                    let _ = write_framed(
                        &mut stream,
                        &json!({"ok": false, "error": "gateway_inventory_control_denied"}),
                    );
                    continue;
                }
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                let response = match handle_control_message(&mut stream, &overlay) {
                    Ok(()) => json!({"ok": true, "reloaded": true}),
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
pub fn serve_inventory_control(_socket_path: PathBuf, _stop: Arc<AtomicBool>) -> Result<()> {
    Err(anyhow!("gateway_inventory_control_unsupported"))
}

/// Push a readiness document to a running sidecar control socket.
#[cfg(unix)]
pub fn apply_inventory_hot(socket_path: &Path, readiness_json: &str) -> Result<()> {
    use std::os::unix::net::UnixStream;

    ensure!(
        socket_path.is_absolute(),
        "gateway_inventory_control_unavailable"
    );
    let mut stream = UnixStream::connect(socket_path)
        .map_err(|_| anyhow!("gateway_inventory_control_unavailable"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    write_framed_bytes(
        std::io::Write::by_ref(&mut stream),
        readiness_json.as_bytes(),
    )?;
    let response = read_framed_json(&mut stream)?;
    if response.get("ok").and_then(Value::as_bool) != Some(true) {
        let code = response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("gateway_inventory_control_failed")
            .to_owned();
        return Err(anyhow!(code));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn apply_inventory_hot(_socket_path: &Path, _readiness_json: &str) -> Result<()> {
    Err(anyhow!("gateway_inventory_control_unsupported"))
}

/// Persist the overlay used by soft-restart fallback and sidecar boot.
pub fn write_inventory_overlay(path: &Path, readiness_json: &str) -> Result<()> {
    ensure!(
        readiness_json.len() <= MAX_CONTROL_BYTES,
        "gateway_inventory_overlay_too_large"
    );
    // Validate before writing so a bad document never poisons boot.
    crate::ports::readiness::reload_document(readiness_json).map_err(|code| anyhow!(code))?;
    if let Some(parent) = path.parent() {
        licoup_foundation::platform::file_security::ensure_private_dir(parent)?;
    }
    licoup_foundation::platform::file_security::atomic_write_private_text(path, readiness_json)?;
    Ok(())
}

/// Load overlay into the live registry when the file exists.
pub fn load_inventory_overlay_if_present(path: &Path) -> Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    crate::ports::readiness::reload_from_path(path).map_err(|code| anyhow!(code))?;
    Ok(true)
}

#[cfg(unix)]
fn handle_control_message(stream: &mut impl Read, overlay: &Path) -> Result<()> {
    let payload = read_framed_bytes(stream)?;
    ensure!(
        !payload.is_empty(),
        "gateway_inventory_control_message_invalid"
    );
    let text = std::str::from_utf8(&payload)
        .map_err(|_| anyhow!("gateway_inventory_control_message_invalid"))?;
    crate::ports::readiness::reload_document(text).map_err(|code| anyhow!(code))?;
    // Persist so soft-restart / next boot keep the hot-applied verified set.
    if let Some(parent) = overlay.parent() {
        licoup_foundation::platform::file_security::ensure_private_dir(parent)?;
    }
    licoup_foundation::platform::file_security::atomic_write_private_text(overlay, text)?;
    Ok(())
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
        "gateway_inventory_control_message_invalid"
    );
    let len = u32::try_from(payload.len())
        .map_err(|_| anyhow!("gateway_inventory_control_message_invalid"))?;
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
        "gateway_inventory_control_message_invalid"
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
        .map_err(|_| anyhow!("gateway_inventory_control_message_invalid"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Readiness application belongs to the composing host; the test installs
    /// a deterministic applier so the pushed document stays observable.
    fn install_recording_readiness() -> Arc<std::sync::Mutex<Vec<String>>> {
        use crate::ports::readiness;
        static APPLIED: std::sync::OnceLock<Arc<std::sync::Mutex<Vec<String>>>> =
            std::sync::OnceLock::new();
        fn apply(text: &str) -> Result<(), &'static str> {
            let parsed: serde_json::Value =
                serde_json::from_str(text).map_err(|_| "readiness_overlay_invalid")?;
            if parsed
                .get("schemaVersion")
                .and_then(serde_json::Value::as_str)
                .is_none()
            {
                return Err("readiness_overlay_invalid");
            }
            APPLIED
                .get()
                .expect("recording applier installed before use")
                .lock()
                .expect("readiness recorder lock")
                .push(text.to_owned());
            Ok(())
        }
        let applied = Arc::new(std::sync::Mutex::new(Vec::new()));
        let _ = APPLIED.set(Arc::clone(&applied));
        let _ = readiness::install(apply);
        applied
    }

    #[test]
    fn inventory_control_hot_applies_and_persists_overlay() {
        use std::os::unix::fs::PermissionsExt;

        let recorded = install_recording_readiness();

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lgw-inv-{stamp}"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = control_socket_path(&root);
        let overlay = overlay_path(&root);
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let server_socket = socket.clone();
        let server = thread::spawn(move || {
            serve_inventory_control(server_socket, server_stop).unwrap();
        });
        for _ in 0..40 {
            if socket.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        assert!(socket.exists());

        // One generated readiness fixture owns the verified document.
        let readiness = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../licoup-native/resources/agent-conversation-readiness.json"
        ));
        let mut applied = false;
        for _ in 0..40 {
            match apply_inventory_hot(&socket, readiness) {
                Ok(()) => {
                    applied = true;
                    break;
                }
                Err(_) => thread::sleep(Duration::from_millis(25)),
            }
        }
        assert!(applied, "inventory hot-apply did not succeed");
        assert!(overlay.is_file());
        let persisted = std::fs::read_to_string(&overlay).unwrap();
        assert!(persisted.contains("client-agent-conversation-readiness-1"));
        assert!(
            recorded
                .lock()
                .expect("readiness recorder lock")
                .iter()
                .any(|text| text.contains("client-agent-conversation-readiness-1")),
            "pushed readiness must reach the installed callback"
        );

        stop.store(true, Ordering::SeqCst);
        let _ = server.join();
        let _ = std::fs::remove_dir_all(&root);
    }
}
