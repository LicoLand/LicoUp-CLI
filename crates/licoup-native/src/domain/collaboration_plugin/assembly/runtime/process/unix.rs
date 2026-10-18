#[cfg(target_os = "macos")]
use anyhow::ensure;
use anyhow::{Result, anyhow};
use sha2::{Digest, Sha256};
use std::path::Path;
#[cfg(target_os = "macos")]
use std::path::PathBuf;

use super::ProcessLiveness;

pub(in crate::domain::collaboration_plugin::assembly) fn capture_identity(
    pid: u32,
) -> Result<String> {
    platform_identity(pid)
}

pub(in crate::domain::collaboration_plugin::assembly) fn liveness(pid: u32) -> ProcessLiveness {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    let Ok(pid) = i32::try_from(pid) else {
        return ProcessLiveness::Unavailable;
    };
    match kill(Pid::from_raw(pid), None) {
        Ok(()) => ProcessLiveness::Alive,
        Err(nix::errno::Errno::ESRCH) => ProcessLiveness::Dead,
        Err(_) => ProcessLiveness::Unavailable,
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn platform_identity(pid: u32) -> Result<String> {
    let executable = std::fs::read_link(format!("/proc/{pid}/exe"))
        .map_err(|_| anyhow!("collaboration_local_server_process_identity_unavailable"))?;
    Ok(format!(
        "linux:{}:{}",
        linux_start_ticks(pid)?,
        path_digest(&executable)
    ))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn linux_start_ticks(pid: u32) -> Result<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map_err(|_| anyhow!("collaboration_local_server_process_identity_unavailable"))?;
    let after_name = text
        .rfind(") ")
        .map(|offset| &text[offset + 2..])
        .ok_or_else(|| anyhow!("collaboration_local_server_process_identity_unavailable"))?;
    after_name
        .split_ascii_whitespace()
        .nth(19)
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| anyhow!("collaboration_local_server_process_identity_unavailable"))
}

#[cfg(target_os = "macos")]
fn platform_identity(pid: u32) -> Result<String> {
    let (path, started_seconds, started_microseconds) = macos_process_facts(pid)?;
    Ok(format!(
        "macos:{started_seconds}:{started_microseconds}:{}",
        path_digest(&path)
    ))
}

#[cfg(target_os = "macos")]
fn macos_process_facts(pid: u32) -> Result<(PathBuf, u64, u64)> {
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};

    const PROC_PIDTBSDINFO: i32 = 3;
    const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;
    unsafe extern "C" {
        fn proc_pidpath(pid: i32, buffer: *mut c_void, buffersize: u32) -> i32;
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut c_void,
            buffersize: i32,
        ) -> i32;
    }
    #[repr(C)]
    struct ProcBsdInfo {
        flags: u32,
        status: u32,
        xstatus: u32,
        pid: u32,
        ppid: u32,
        uid: u32,
        gid: u32,
        ruid: u32,
        rgid: u32,
        svuid: u32,
        svgid: u32,
        reserved: u32,
        comm: [u8; 16],
        name: [u8; 32],
        nfiles: u32,
        pgid: u32,
        pjobc: u32,
        e_tdev: u32,
        e_tpgid: u32,
        nice: i32,
        start_seconds: u64,
        start_microseconds: u64,
    }
    let pid = i32::try_from(pid).map_err(|_| anyhow!("collaboration_local_server_pid_invalid"))?;
    let mut path = [0u8; PROC_PIDPATHINFO_MAXSIZE];
    let path_len = unsafe {
        // SAFETY: the fixed buffer is writable for the provided size.
        proc_pidpath(pid, path.as_mut_ptr().cast::<c_void>(), path.len() as u32)
    };
    ensure!(
        path_len > 0,
        "collaboration_local_server_process_identity_unavailable"
    );
    let path_len = usize::try_from(path_len)
        .map_err(|_| anyhow!("collaboration_local_server_process_identity_unavailable"))?;
    let mut info: ProcBsdInfo = unsafe { zeroed() };
    let info_len = unsafe {
        // SAFETY: info is writable and its exact layout mirrors proc_bsdinfo.
        proc_pidinfo(
            pid,
            PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut ProcBsdInfo).cast::<c_void>(),
            size_of::<ProcBsdInfo>() as i32,
        )
    };
    ensure!(
        info_len == size_of::<ProcBsdInfo>() as i32 && info.pid == pid as u32,
        "collaboration_local_server_process_identity_unavailable"
    );
    Ok((
        PathBuf::from(
            std::str::from_utf8(&path[..path_len])
                .map_err(|_| anyhow!("collaboration_local_server_process_identity_unavailable"))?,
        ),
        info.start_seconds,
        info.start_microseconds,
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn platform_identity(_pid: u32) -> Result<String> {
    Err(anyhow!(
        "collaboration_local_server_process_identity_unavailable"
    ))
}

fn path_digest(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    format!("{:x}", Sha256::digest(path.as_os_str().as_bytes()))
}
