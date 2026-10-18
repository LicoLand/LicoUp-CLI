use anyhow::{Result, anyhow, ensure};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use crate::platform::client_state::ClientStateStore;

pub(super) struct ImmutableRuntimeFile {
    file: File,
    execution_path: PathBuf,
    #[cfg(target_os = "macos")]
    cleanup_path: Option<PathBuf>,
}

impl ImmutableRuntimeFile {
    pub(super) fn from_verified_bytes(
        store: &ClientStateStore,
        bytes: &[u8],
        executable: bool,
    ) -> Result<Self> {
        ensure!(
            !bytes.is_empty(),
            "collaboration_local_server_immutable_input_invalid"
        );
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            let _ = store;
            return linux_memfd(bytes, executable);
        }
        #[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
        {
            return unix_unlinked_file(store, bytes, executable);
        }
        #[cfg(not(unix))]
        {
            let _ = (store, bytes, executable);
            Err(anyhow!(
                "collaboration_local_server_immutable_execution_unavailable"
            ))
        }
    }

    pub(super) fn path(&self) -> &std::path::Path {
        &self.execution_path
    }

    pub(super) fn validate(&mut self, expected: &[u8]) -> Result<()> {
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|_| anyhow!("collaboration_local_server_immutable_input_changed"))?;
        let mut actual = Vec::with_capacity(expected.len());
        Read::by_ref(&mut self.file)
            .take((expected.len() as u64).saturating_add(1))
            .read_to_end(&mut actual)
            .map_err(|_| anyhow!("collaboration_local_server_immutable_input_changed"))?;
        ensure!(
            actual == expected,
            "collaboration_local_server_immutable_input_changed"
        );
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|_| anyhow!("collaboration_local_server_immutable_input_changed"))?;
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn linux_memfd(bytes: &[u8], executable: bool) -> Result<ImmutableRuntimeFile> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};

    let name = CString::new("licoup-local-server-runtime")?;
    let raw = unsafe {
        // SAFETY: the name is NUL-terminated and flags are fixed memfd flags.
        nix::libc::syscall(
            nix::libc::SYS_memfd_create,
            name.as_ptr(),
            nix::libc::MFD_ALLOW_SEALING,
        )
    };
    ensure!(
        raw >= 0,
        "collaboration_local_server_immutable_execution_unavailable"
    );
    let mut file = unsafe {
        // SAFETY: a successful memfd_create returns one newly-owned descriptor.
        File::from_raw_fd(raw as i32)
    };
    file.write_all(bytes)
        .map_err(|_| anyhow!("collaboration_local_server_immutable_input_write_failed"))?;
    file.sync_all()
        .map_err(|_| anyhow!("collaboration_local_server_immutable_input_write_failed"))?;
    let mode = if executable { 0o500 } else { 0o400 };
    let changed = unsafe {
        // SAFETY: file owns a live descriptor and the fixed mode is valid.
        nix::libc::fchmod(file.as_raw_fd(), mode)
    };
    ensure!(
        changed == 0,
        "collaboration_local_server_immutable_input_write_failed"
    );
    let seals = nix::libc::F_SEAL_SEAL
        | nix::libc::F_SEAL_SHRINK
        | nix::libc::F_SEAL_GROW
        | nix::libc::F_SEAL_WRITE;
    let sealed = unsafe {
        // SAFETY: file is a sealing-enabled memfd and seals is a fixed mask.
        nix::libc::fcntl(file.as_raw_fd(), nix::libc::F_ADD_SEALS, seals)
    };
    ensure!(
        sealed == 0,
        "collaboration_local_server_immutable_execution_unavailable"
    );
    clear_close_on_exec(&file)?;
    let execution_path = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
    let mut value = ImmutableRuntimeFile {
        file,
        execution_path,
        #[cfg(target_os = "macos")]
        cleanup_path: None,
    };
    value.validate(bytes)?;
    Ok(value)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn unix_unlinked_file(
    store: &ClientStateStore,
    bytes: &[u8],
    executable: bool,
) -> Result<ImmutableRuntimeFile> {
    use std::fs::{self, OpenOptions};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let root = store.root().join(".local-server-runtime-images");
    licoup_foundation::platform::file_security::ensure_private_dir(&root)?;
    let path = root.join(format!("image-{}", uuid::Uuid::new_v4()));
    let mut writer = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .map_err(|_| anyhow!("collaboration_local_server_immutable_input_write_failed"))?;
    let result = (|| -> Result<ImmutableRuntimeFile> {
        writer
            .write_all(bytes)
            .map_err(|_| anyhow!("collaboration_local_server_immutable_input_write_failed"))?;
        writer
            .sync_all()
            .map_err(|_| anyhow!("collaboration_local_server_immutable_input_write_failed"))?;
        fs::set_permissions(
            &path,
            fs::Permissions::from_mode(if executable { 0o500 } else { 0o400 }),
        )?;
        let file = OpenOptions::new()
            .read(true)
            .open(&path)
            .map_err(|_| anyhow!("collaboration_local_server_immutable_input_write_failed"))?;
        drop(writer);
        clear_close_on_exec(&file)?;
        #[cfg(target_os = "macos")]
        let cleanup_path = if executable {
            set_macos_user_immutable(&file)?;
            Some(path.clone())
        } else {
            fs::remove_file(&path).map_err(|_| {
                anyhow!("collaboration_local_server_immutable_execution_unavailable")
            })?;
            None
        };
        #[cfg(not(target_os = "macos"))]
        fs::remove_file(&path)
            .map_err(|_| anyhow!("collaboration_local_server_immutable_execution_unavailable"))?;
        #[cfg(target_os = "macos")]
        let execution_path = cleanup_path
            .clone()
            .unwrap_or_else(|| PathBuf::from(format!("/dev/fd/{}", file.as_raw_fd())));
        #[cfg(not(target_os = "macos"))]
        let execution_path = PathBuf::from(format!("/dev/fd/{}", file.as_raw_fd()));
        let mut value = ImmutableRuntimeFile {
            file,
            execution_path,
            #[cfg(target_os = "macos")]
            cleanup_path,
        };
        value.validate(bytes)?;
        Ok(value)
    })();
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

#[cfg(target_os = "macos")]
fn set_macos_user_immutable(file: &File) -> Result<()> {
    use std::os::fd::AsRawFd;
    let changed = unsafe {
        // SAFETY: file owns a live descriptor and UF_IMMUTABLE is a fixed,
        // owner-manageable vnode flag applied before the executable path is
        // exposed to the process launcher.
        nix::libc::fchflags(file.as_raw_fd(), nix::libc::UF_IMMUTABLE)
    };
    ensure!(
        changed == 0,
        "collaboration_local_server_immutable_execution_unavailable"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
impl Drop for ImmutableRuntimeFile {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        if let Some(path) = self.cleanup_path.take() {
            unsafe {
                // SAFETY: this instance created the file and retains its live
                // descriptor; clearing the owner flag is required only for the
                // final unlink after spawn has consumed the executable path.
                nix::libc::fchflags(self.file.as_raw_fd(), 0);
            }
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(unix)]
fn clear_close_on_exec(file: &File) -> Result<()> {
    use std::os::fd::AsRawFd;
    let flags = unsafe {
        // SAFETY: file owns a live descriptor; F_GETFD does not mutate memory.
        nix::libc::fcntl(file.as_raw_fd(), nix::libc::F_GETFD)
    };
    ensure!(
        flags >= 0,
        "collaboration_local_server_immutable_execution_unavailable"
    );
    let changed = unsafe {
        // SAFETY: file owns a live descriptor and flags only clears FD_CLOEXEC.
        nix::libc::fcntl(
            file.as_raw_fd(),
            nix::libc::F_SETFD,
            flags & !nix::libc::FD_CLOEXEC,
        )
    };
    ensure!(
        changed == 0,
        "collaboration_local_server_immutable_execution_unavailable"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ImmutableRuntimeFile;
    use crate::platform::client_state::ClientStateStore;
    use std::process::Command;
    use uuid::Uuid;

    #[cfg(unix)]
    #[test]
    fn copied_executable_is_immutable_and_survives_source_buffer_mutation() {
        let root =
            std::env::temp_dir().join(format!("licoup-immutable-runtime-test-{}", Uuid::new_v4()));
        let store = ClientStateStore::new(root.clone()).unwrap();
        let mut source = std::fs::read("/usr/bin/true").unwrap();
        let expected = source.clone();
        let mut image = ImmutableRuntimeFile::from_verified_bytes(&store, &source, true).unwrap();
        source.fill(0);

        image.validate(&expected).unwrap();
        #[cfg(target_os = "macos")]
        {
            let replacement = root.join("replacement");
            std::fs::write(&replacement, b"replacement").unwrap();
            assert!(std::fs::rename(&replacement, image.path()).is_err());
            std::fs::remove_file(replacement).unwrap();
        }
        let status = Command::new(image.path()).status().unwrap();
        assert!(status.success());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn empty_runtime_image_is_rejected() {
        let root =
            std::env::temp_dir().join(format!("licoup-empty-runtime-test-{}", Uuid::new_v4()));
        let store = ClientStateStore::new(root.clone()).unwrap();
        assert!(ImmutableRuntimeFile::from_verified_bytes(&store, &[], false).is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
