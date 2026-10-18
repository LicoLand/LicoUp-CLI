use anyhow::{Result, anyhow, ensure};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INVALID_PARAMETER, FILETIME, GetLastError, HANDLE, STILL_ACTIVE,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};

use super::ProcessLiveness;

pub(in crate::domain::collaboration_plugin::assembly) fn capture_identity(
    pid: u32,
) -> Result<String> {
    let handle = ProcessHandle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    identity_on_handle(&handle)
}

pub(in crate::domain::collaboration_plugin::assembly) fn liveness(pid: u32) -> ProcessLiveness {
    let handle = match ProcessHandle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION) {
        Ok(handle) => handle,
        Err(_) => {
            let error = unsafe {
                // SAFETY: GetLastError reads the calling thread's last-error slot.
                GetLastError()
            };
            return if error == ERROR_INVALID_PARAMETER {
                ProcessLiveness::Dead
            } else {
                ProcessLiveness::Unavailable
            };
        }
    };
    let mut exit_code = 0u32;
    let result = unsafe {
        // SAFETY: exit_code is writable and handle remains valid for this call.
        GetExitCodeProcess(handle.0, &mut exit_code)
    };
    if result == 0 {
        ProcessLiveness::Unavailable
    } else if exit_code == STILL_ACTIVE as u32 {
        ProcessLiveness::Alive
    } else {
        ProcessLiveness::Dead
    }
}

fn identity_on_handle(handle: &ProcessHandle) -> Result<String> {
    let actual = executable_path(handle)?;
    let encoded = actual
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_be_bytes)
        .collect::<Vec<_>>();
    Ok(format!(
        "windows:{}:{:x}",
        creation_time(handle)?,
        Sha256::digest(encoded)
    ))
}

fn executable_path(handle: &ProcessHandle) -> Result<PathBuf> {
    let mut buffer = vec![0u16; 32_768];
    let mut length = buffer.len() as u32;
    let result = unsafe {
        // SAFETY: buffer is writable for length UTF-16 units and handle is valid.
        QueryFullProcessImageNameW(handle.0, 0, buffer.as_mut_ptr(), &mut length)
    };
    ensure!(
        result != 0 && length > 0 && length as usize <= buffer.len(),
        "collaboration_local_server_process_identity_unavailable"
    );
    buffer.truncate(length as usize);
    Ok(PathBuf::from(OsString::from_wide(&buffer)))
}

fn creation_time(handle: &ProcessHandle) -> Result<u64> {
    let mut created = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exited = created;
    let mut kernel = created;
    let mut user = created;
    let result = unsafe {
        // SAFETY: all FILETIME outputs are writable and handle is valid.
        GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user)
    };
    ensure!(
        result != 0,
        "collaboration_local_server_process_identity_unavailable"
    );
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

struct ProcessHandle(HANDLE);

impl ProcessHandle {
    fn open(pid: u32, access: u32) -> Result<Self> {
        let handle = unsafe {
            // SAFETY: OpenProcess receives a numeric PID and a bounded access mask.
            OpenProcess(access, 0, pid)
        };
        ensure!(
            !handle.is_null(),
            "collaboration_local_server_process_identity_unavailable"
        );
        Ok(Self(handle))
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: this wrapper owns exactly one non-null process handle.
            CloseHandle(self.0);
        }
    }
}
