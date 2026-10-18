use anyhow::{Result, anyhow};
use fs2::FileExt;
use licoup_foundation::platform::file_security::{
    atomic_write_private_text_bounded, ensure_private_dir, open_private_lock_file,
    read_private_text_bounded, remove_private_state_marker,
};
use licoup_foundation::platform::paths;
use serde_json::{Value, json};
use std::fs::File;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Instant;

use super::bounds::{
    MAX_PID_BYTES, MAX_PRIVATE_STATE_BYTES, OPERATION_LOCK_WAIT, PROCESS_POLL_INTERVAL,
};

#[derive(Clone, Debug)]
pub struct ServicePaths {
    pub root: PathBuf,
    pub state_path: PathBuf,
    pub pid_path: PathBuf,
    pub lock_path: PathBuf,
}

impl ServicePaths {
    pub fn resolve(state_dir: &str, pid_name: &str) -> Result<Self> {
        Self::from_root(paths::portable_data_dir()?.join(state_dir), pid_name)
    }

    pub fn from_root(root: PathBuf, pid_name: &str) -> Result<Self> {
        ensure_private_dir(&root)?;
        Ok(Self {
            state_path: root.join("state.json"),
            pid_path: root.join(pid_name),
            lock_path: root.join("operation.lock"),
            root,
        })
    }
}

pub struct OperationLock {
    file: File,
}

impl OperationLock {
    pub fn acquire(path: &Path) -> Result<Self> {
        let file = open_private_lock_file(path)?;
        let deadline = Instant::now() + OPERATION_LOCK_WAIT;
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self { file }),
                Err(error)
                    if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(PROCESS_POLL_INTERVAL);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    return Err(anyhow!("local_service_operation_busy"));
                }
                Err(_) => return Err(anyhow!("local_service_operation_lock_failed")),
            }
        }
    }
}

impl Drop for OperationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub fn read_json(path: &Path, invalid_code: &'static str) -> Result<Value> {
    let Some(text) = read_private_text_bounded(path, MAX_PRIVATE_STATE_BYTES)? else {
        return Ok(json!({}));
    };
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).map_err(|_| anyhow!(invalid_code))
}

pub fn write_json(path: &Path, value: &Value) -> Result<()> {
    let body = serde_json::to_string_pretty(value)?;
    atomic_write_private_text_bounded(path, &body, MAX_PRIVATE_STATE_BYTES)
}

pub fn read_pid(path: &Path) -> Result<Option<u32>> {
    let Some(text) = read_private_text_bounded(path, MAX_PID_BYTES)? else {
        return Ok(None);
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let pid = trimmed
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid != 0)
        .ok_or_else(|| anyhow!("local_service_pid_invalid"))?;
    Ok(Some(pid))
}

pub fn write_pid(path: &Path, pid: u32) -> Result<()> {
    if pid == 0 {
        return Err(anyhow!("local_service_pid_invalid"));
    }
    atomic_write_private_text_bounded(path, &format!("{}\n", pid), MAX_PID_BYTES)
}

pub fn remove_pid(path: &Path) -> Result<bool> {
    remove_private_state_marker(path)
}
