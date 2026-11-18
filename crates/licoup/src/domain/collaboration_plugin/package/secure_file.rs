use anyhow::{Result, anyhow, ensure};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Take};
use std::path::Path;

pub(in crate::domain::collaboration_plugin) fn read_file_no_follow(
    path: &Path,
    maximum_bytes: usize,
) -> Result<Vec<u8>> {
    let before = fs::symlink_metadata(path)
        .map_err(|_| anyhow!("collaboration_plugin_package_entry_unavailable"))?;
    ensure!(
        before.file_type().is_file() && !before.file_type().is_symlink(),
        "collaboration_plugin_package_entry_type_rejected"
    );
    // Windows exposes no stable `std` accessor for the volume serial number,
    // link count, or file index, so the object identity is bound through
    // `GetFileInformationByHandle` on a no-follow handle instead.
    #[cfg(windows)]
    let identity_before = windows_identity::bind_path(path)?;
    let mut file = open_no_follow(path)?;
    #[cfg(windows)]
    ensure!(
        windows_identity::identity_of(&file)? == identity_before,
        "collaboration_plugin_package_file_changed"
    );
    let opened = file
        .metadata()
        .map_err(|_| anyhow!("collaboration_plugin_package_file_read_failed"))?;
    validate_same_private_file(&before, &opened)?;
    let declared = usize::try_from(opened.len())
        .map_err(|_| anyhow!("collaboration_plugin_package_file_too_large"))?;
    ensure!(
        declared <= maximum_bytes,
        "collaboration_plugin_package_too_large"
    );
    let mut bytes = Vec::with_capacity(declared);
    bounded_reader(&mut file, maximum_bytes).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() == declared && bytes.len() <= maximum_bytes,
        "collaboration_plugin_package_file_changed"
    );
    let after = fs::symlink_metadata(path)
        .map_err(|_| anyhow!("collaboration_plugin_package_file_changed"))?;
    let opened_after = file
        .metadata()
        .map_err(|_| anyhow!("collaboration_plugin_package_file_changed"))?;
    validate_same_private_file(&before, &after)?;
    validate_same_private_file(&opened, &opened_after)?;
    #[cfg(windows)]
    {
        let identity_after = windows_identity::bind_path(path)?;
        let opened_identity_after = windows_identity::identity_of(&file)?;
        ensure!(
            identity_after == identity_before && opened_identity_after == identity_before,
            "collaboration_plugin_package_file_changed"
        );
    }
    Ok(bytes)
}

fn bounded_reader(file: &mut File, maximum_bytes: usize) -> Take<&mut File> {
    file.take(maximum_bytes.saturating_add(1) as u64)
}

/// One file object's identity facts: the volume it lives on, its file index on
/// that volume, and its hard-link count.
#[cfg(windows)]
mod windows_identity {
    use anyhow::{Result, anyhow, ensure};
    use std::fs::{File, OpenOptions};
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    #[derive(Clone, Copy, Eq, PartialEq)]
    pub(super) struct FileIdentity {
        volume_serial_number: u32,
        file_index: u64,
        number_of_links: u32,
    }

    pub(super) fn identity_of(file: &File) -> Result<FileIdentity> {
        let mut information: BY_HANDLE_FILE_INFORMATION = unsafe {
            // SAFETY: a zeroed BY_HANDLE_FILE_INFORMATION is a valid initial
            // value for the out-parameter below.
            std::mem::zeroed()
        };
        let result = unsafe {
            // SAFETY: `file` owns a live handle and `information` is a
            // correctly sized, writable out-parameter.
            GetFileInformationByHandle(file.as_raw_handle(), &mut information)
        };
        ensure!(result != 0, "collaboration_plugin_package_file_read_failed");
        let identity = FileIdentity {
            volume_serial_number: information.dwVolumeSerialNumber,
            file_index: (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow),
            number_of_links: information.nNumberOfLinks,
        };
        ensure!(
            identity.number_of_links == 1,
            "collaboration_plugin_package_file_changed"
        );
        Ok(identity)
    }

    /// Binds one path to the exact file object it currently names without
    /// following a final reparse point.
    pub(super) fn bind_path(path: &Path) -> Result<FileIdentity> {
        let handle = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|_| anyhow!("collaboration_plugin_package_file_read_failed"))?;
        let attributes = handle
            .metadata()
            .map_err(|_| anyhow!("collaboration_plugin_package_file_read_failed"))?
            .file_attributes();
        ensure!(
            attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0,
            "collaboration_plugin_package_entry_type_rejected"
        );
        identity_of(&handle)
    }
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| anyhow!("collaboration_plugin_package_file_read_failed"))
}

#[cfg(windows)]
fn open_no_follow(path: &Path) -> Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|_| anyhow!("collaboration_plugin_package_file_read_failed"))
}

#[cfg(not(any(unix, windows)))]
fn open_no_follow(_path: &Path) -> Result<File> {
    Err(anyhow!("collaboration_plugin_package_platform_unsupported"))
}

#[cfg(unix)]
fn validate_same_private_file(left: &fs::Metadata, right: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    ensure!(
        left.file_type().is_file()
            && right.file_type().is_file()
            && left.nlink() == 1
            && right.nlink() == 1
            && left.dev() == right.dev()
            && left.ino() == right.ino()
            && left.len() == right.len()
            && left.mtime() == right.mtime()
            && left.mtime_nsec() == right.mtime_nsec()
            && left.ctime() == right.ctime()
            && left.ctime_nsec() == right.ctime_nsec(),
        "collaboration_plugin_package_file_changed"
    );
    Ok(())
}

/// Stable-field comparison for one file object.
///
/// Volume serial number, file index, and link count are not available through
/// stable `std` accessors on Windows; [`windows_identity`] owns those facts and
/// the caller compares them separately.
#[cfg(windows)]
fn validate_same_private_file(left: &fs::Metadata, right: &fs::Metadata) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    ensure!(
        left.file_type().is_file()
            && right.file_type().is_file()
            && left.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
            && right.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
            && left.file_size() == right.file_size()
            && left.creation_time() == right.creation_time()
            && left.last_write_time() == right.last_write_time(),
        "collaboration_plugin_package_file_changed"
    );
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn validate_same_private_file(_left: &fs::Metadata, _right: &fs::Metadata) -> Result<()> {
    Err(anyhow!("collaboration_plugin_package_platform_unsupported"))
}
