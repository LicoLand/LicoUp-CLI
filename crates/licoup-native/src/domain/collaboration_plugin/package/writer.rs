use super::inspection::{inspect_package, validate_relative_components};
use super::{InspectedPackage, SelectedPayloadFile};
use anyhow::{Result, anyhow, ensure};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

pub(in crate::domain::collaboration_plugin) fn write_inspected_package(
    package: &InspectedPackage,
    destination: &Path,
) -> Result<()> {
    write_new_private_tree(
        destination,
        package
            .files
            .iter()
            .map(|file| (file.relative_path.as_path(), file.bytes.as_slice())),
    )?;
    let installed = inspect_package(destination)?;
    ensure!(
        installed.digest_sha256 == package.digest_sha256,
        "collaboration_plugin_installed_digest_mismatch"
    );
    Ok(())
}

pub(in crate::domain::collaboration_plugin) fn write_selected_payload_tree(
    files: &[SelectedPayloadFile],
    destination: &Path,
) -> Result<()> {
    write_new_private_tree(
        destination,
        files.iter().map(|file| {
            (
                file.destination_relative_path.as_path(),
                file.bytes.as_slice(),
            )
        }),
    )
}

fn write_new_private_tree<'a>(
    destination: &Path,
    files: impl IntoIterator<Item = (&'a Path, &'a [u8])>,
) -> Result<()> {
    write_new_private_tree_with_hook(destination, files, || Ok(()))
}

fn write_new_private_tree_with_hook<'a>(
    destination: &Path,
    files: impl IntoIterator<Item = (&'a Path, &'a [u8])>,
    after_root_created: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let tree = SecureNewTree::create(destination)?;
    let result = (|| -> Result<()> {
        tree.sync_and_validate_binding()
            .map_err(|_| anyhow!("collaboration_plugin_destination_initial_binding_invalid"))?;
        after_root_created()?;
        for (relative_path, bytes) in files {
            validate_relative_components(relative_path)?;
            tree.write_file(relative_path, bytes)
                .map_err(|_| anyhow!("collaboration_plugin_package_secure_write_failed"))?;
            tree.verify_file(relative_path, bytes)
                .map_err(|_| anyhow!("collaboration_plugin_package_secure_verify_failed"))?;
        }
        tree.sync_and_validate_binding()
            .map_err(|_| anyhow!("collaboration_plugin_destination_final_binding_invalid"))
    })();
    if result.is_err() {
        tree.remove_if_still_bound();
    }
    result
}

#[cfg(test)]
pub(in crate::domain::collaboration_plugin) fn write_inspected_package_with_hook(
    package: &InspectedPackage,
    destination: &Path,
    after_root_created: impl FnOnce() -> Result<()>,
) -> Result<()> {
    write_new_private_tree_with_hook(
        destination,
        package
            .files
            .iter()
            .map(|file| (file.relative_path.as_path(), file.bytes.as_slice())),
        after_root_created,
    )?;
    let installed = inspect_package(destination)?;
    ensure!(
        installed.digest_sha256 == package.digest_sha256,
        "collaboration_plugin_installed_digest_mismatch"
    );
    Ok(())
}

/// A newly-created owner-only directory held open while every descendant is
/// created relative to no-follow directory handles. This prevents a package
/// path from being redirected through a concurrently replaced symbolic link.
pub(in crate::domain::collaboration_plugin) struct SecureNewTree {
    path: PathBuf,
    #[cfg(unix)]
    parent: fs::File,
    #[cfg(unix)]
    root: fs::File,
}

impl SecureNewTree {
    pub(in crate::domain::collaboration_plugin) fn create(path: &Path) -> Result<Self> {
        let parent_path = path
            .parent()
            .ok_or_else(|| anyhow!("collaboration_plugin_destination_parent_missing"))?;
        let leaf = path
            .file_name()
            .ok_or_else(|| anyhow!("collaboration_plugin_destination_invalid"))?;

        #[cfg(unix)]
        {
            let parent = open_directory_path_no_follow(parent_path)?;
            let leaf = component_cstring(leaf)?;
            // SAFETY: `parent` owns a live directory descriptor and `leaf` is a
            // validated NUL-free component; the syscall result is checked.
            let created = unsafe { nix::libc::mkdirat(parent.as_raw_fd(), leaf.as_ptr(), 0o700) };
            ensure!(created == 0, "collaboration_plugin_destination_must_be_new");
            let root = match open_directory_at(&parent, &leaf) {
                Ok(root) => root,
                Err(error) => {
                    // SAFETY: the directory descriptor and component remain live;
                    // this best-effort cleanup removes only the directory just made.
                    let _ = unsafe {
                        nix::libc::unlinkat(
                            parent.as_raw_fd(),
                            leaf.as_ptr(),
                            nix::libc::AT_REMOVEDIR,
                        )
                    };
                    return Err(error);
                }
            };
            return Ok(Self {
                path: path.to_path_buf(),
                parent,
                root,
            });
        }

        #[cfg(not(unix))]
        {
            licoup_foundation::platform::file_security::validate_no_symlink_ancestors(parent_path)?;
            let parent_metadata = fs::symlink_metadata(parent_path)
                .map_err(|_| anyhow!("collaboration_plugin_destination_parent_missing"))?;
            ensure!(
                parent_metadata.is_dir() && !parent_metadata.file_type().is_symlink(),
                "collaboration_plugin_destination_parent_invalid"
            );
            fs::create_dir(path)
                .map_err(|_| anyhow!("collaboration_plugin_destination_must_be_new"))?;
            licoup_foundation::platform::file_security::harden_private_path(path)?;
            return Ok(Self {
                path: path.to_path_buf(),
            });
        }
    }

    pub(in crate::domain::collaboration_plugin) fn write_file(
        &self,
        relative_path: &Path,
        bytes: &[u8],
    ) -> Result<()> {
        #[cfg(unix)]
        {
            let parent = open_or_create_relative_parent(&self.root, relative_path)?;
            let leaf = component_cstring(
                relative_path
                    .file_name()
                    .ok_or_else(|| anyhow!("collaboration_plugin_package_path_invalid"))?,
            )?;
            let flags = nix::libc::O_WRONLY
                | nix::libc::O_CREAT
                | nix::libc::O_EXCL
                | nix::libc::O_NOFOLLOW
                | nix::libc::O_CLOEXEC;
            // SAFETY: the live directory descriptor and validated component stay
            // valid for `openat`; the result is checked before ownership.
            let fd = unsafe { nix::libc::openat(parent.as_raw_fd(), leaf.as_ptr(), flags, 0o600) };
            ensure!(fd >= 0, "collaboration_plugin_package_file_must_be_new");
            // SAFETY: the successful `openat` returned a new descriptor owned here.
            let mut file = unsafe { fs::File::from_raw_fd(fd) };
            file.write_all(bytes)
                .map_err(|_| anyhow!("collaboration_plugin_package_write_failed"))?;
            file.sync_all()
                .map_err(|_| anyhow!("collaboration_plugin_package_sync_failed"))?;
            let metadata = file.metadata()?;
            ensure!(
                metadata.is_file() && metadata.len() == bytes.len() as u64,
                "collaboration_plugin_package_file_verification_failed"
            );
            return Ok(());
        }

        #[cfg(not(unix))]
        {
            let output = self.path.join(relative_path);
            let parent = output
                .parent()
                .ok_or_else(|| anyhow!("collaboration_plugin_package_path_invalid"))?;
            create_directory_path_no_follow(parent)?;
            ensure!(
                fs::symlink_metadata(&output).is_err(),
                "collaboration_plugin_package_file_must_be_new"
            );
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
                options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
            }
            let mut file = options
                .open(&output)
                .map_err(|_| anyhow!("collaboration_plugin_package_file_must_be_new"))?;
            file.write_all(bytes)
                .map_err(|_| anyhow!("collaboration_plugin_package_write_failed"))?;
            file.sync_all()
                .map_err(|_| anyhow!("collaboration_plugin_package_sync_failed"))?;
            set_private_file_permissions(&output)?;
            let metadata = file.metadata()?;
            let path_metadata = fs::symlink_metadata(&output)?;
            ensure!(
                metadata.is_file()
                    && path_metadata.is_file()
                    && !path_metadata.file_type().is_symlink()
                    && metadata.len() == bytes.len() as u64,
                "collaboration_plugin_package_file_verification_failed"
            );
            Ok(())
        }
    }

    pub(in crate::domain::collaboration_plugin) fn verify_file(
        &self,
        relative_path: &Path,
        expected: &[u8],
    ) -> Result<()> {
        #[cfg(unix)]
        let mut file = {
            let parent = open_existing_relative_parent(&self.root, relative_path)?;
            let leaf = component_cstring(
                relative_path
                    .file_name()
                    .ok_or_else(|| anyhow!("collaboration_plugin_package_path_invalid"))?,
            )?;
            let flags = nix::libc::O_RDONLY | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC;
            // SAFETY: the live directory descriptor and validated component stay
            // valid for `openat`; the result is checked before ownership.
            let fd = unsafe { nix::libc::openat(parent.as_raw_fd(), leaf.as_ptr(), flags) };
            ensure!(fd >= 0, "collaboration_plugin_package_file_changed");
            // SAFETY: the successful `openat` returned a new descriptor owned here.
            unsafe { fs::File::from_raw_fd(fd) }
        };
        #[cfg(not(unix))]
        let mut file = {
            let path = self.path.join(relative_path);
            licoup_foundation::platform::file_security::validate_no_symlink_ancestors(&path)?;
            OpenOptions::new()
                .read(true)
                .open(path)
                .map_err(|_| anyhow!("collaboration_plugin_package_file_changed"))?
        };
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file() && metadata.len() == expected.len() as u64,
            "collaboration_plugin_package_file_changed"
        );
        let mut actual = Vec::with_capacity(expected.len());
        Read::by_ref(&mut file)
            .take((expected.len() as u64).saturating_add(1))
            .read_to_end(&mut actual)?;
        ensure!(
            actual == expected,
            "collaboration_plugin_package_file_changed"
        );
        Ok(())
    }

    pub(in crate::domain::collaboration_plugin) fn create_directory(
        &self,
        relative_path: &Path,
    ) -> Result<()> {
        validate_relative_components(relative_path)?;
        #[cfg(unix)]
        {
            let mut current = self.root.try_clone()?;
            for component in relative_path.components() {
                let Component::Normal(name) = component else {
                    return Err(anyhow!("collaboration_plugin_package_path_invalid"));
                };
                current = open_or_create_directory_at(&current, name)?;
            }
            current.sync_all()?;
        }
        #[cfg(not(unix))]
        {
            create_directory_path_no_follow(&self.path.join(relative_path))?;
        }
        self.sync_and_validate_binding()
    }

    pub(in crate::domain::collaboration_plugin) fn make_file_owner_executable(
        &self,
        relative_path: &Path,
    ) -> Result<()> {
        validate_relative_components(relative_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let parent = open_existing_relative_parent(&self.root, relative_path)?;
            let leaf = component_cstring(
                relative_path
                    .file_name()
                    .ok_or_else(|| anyhow!("collaboration_plugin_package_path_invalid"))?,
            )?;
            let flags = nix::libc::O_RDONLY | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC;
            // SAFETY: parent and component are live and validated; the result
            // is checked before ownership is assumed.
            let fd = unsafe { nix::libc::openat(parent.as_raw_fd(), leaf.as_ptr(), flags) };
            ensure!(fd >= 0, "collaboration_plugin_package_file_changed");
            // SAFETY: the successful openat returned a newly-owned descriptor.
            let file = unsafe { fs::File::from_raw_fd(fd) };
            // SAFETY: file owns a live descriptor and the fixed mode grants no
            // group/other access.
            let changed = unsafe { nix::libc::fchmod(file.as_raw_fd(), 0o700) };
            ensure!(
                changed == 0,
                "collaboration_plugin_package_permission_failed"
            );
            let metadata = file.metadata()?;
            ensure!(
                metadata.is_file() && metadata.nlink() == 1,
                "collaboration_plugin_package_file_changed"
            );
            file.sync_all()?;
        }
        #[cfg(not(unix))]
        {
            let path = self.path.join(relative_path);
            licoup_foundation::platform::file_security::validate_no_symlink_ancestors(&path)?;
            licoup_foundation::platform::file_security::harden_private_path(&path)?;
        }
        self.sync_and_validate_binding()
    }

    pub(in crate::domain::collaboration_plugin) fn sync_and_validate_binding(&self) -> Result<()> {
        #[cfg(unix)]
        {
            self.root.sync_all()?;
            self.parent.sync_all()?;
            use std::os::unix::fs::MetadataExt;
            let path_metadata = fs::symlink_metadata(&self.path)
                .map_err(|_| anyhow!("collaboration_plugin_destination_changed"))?;
            let root_metadata = self.root.metadata()?;
            ensure!(
                path_metadata.is_dir() && !path_metadata.file_type().is_symlink(),
                "collaboration_plugin_destination_type_changed"
            );
            ensure!(
                path_metadata.dev() == root_metadata.dev(),
                "collaboration_plugin_destination_device_changed"
            );
            ensure!(
                path_metadata.ino() == root_metadata.ino(),
                "collaboration_plugin_destination_inode_changed"
            );
        }
        #[cfg(not(unix))]
        {
            licoup_foundation::platform::file_security::validate_no_symlink_ancestors(&self.path)?;
            let metadata = fs::symlink_metadata(&self.path)?;
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "collaboration_plugin_destination_changed"
            );
        }
        Ok(())
    }

    pub(in crate::domain::collaboration_plugin) fn remove_if_still_bound(&self) -> bool {
        if self.sync_and_validate_binding().is_ok() {
            return fs::remove_dir_all(&self.path).is_ok();
        }
        false
    }
}

#[cfg(unix)]
pub(in crate::domain::collaboration_plugin) fn open_directory_path_no_follow(
    path: &Path,
) -> Result<fs::File> {
    use std::os::unix::fs::MetadataExt;

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    ensure!(
        !absolute.components().any(|component| matches!(
            component,
            Component::CurDir | Component::ParentDir | Component::Prefix(_)
        )),
        "collaboration_plugin_destination_path_invalid"
    );
    let snapshots = trusted_path_snapshots(&absolute)?;
    let canonical = fs::canonicalize(&absolute)
        .map_err(|_| anyhow!("collaboration_plugin_destination_parent_invalid"))?;
    let directory = open_canonical_directory_no_follow(&canonical)?;
    for (component, device, inode, symlink) in snapshots {
        let metadata = fs::symlink_metadata(component)
            .map_err(|_| anyhow!("collaboration_plugin_destination_parent_changed"))?;
        ensure!(
            metadata.dev() == device
                && metadata.ino() == inode
                && metadata.file_type().is_symlink() == symlink,
            "collaboration_plugin_destination_parent_changed"
        );
    }
    Ok(directory)
}

/// Opens one existing directory without following a final reparse point.
///
/// Windows has no `openat` chain, so the bound object is established by
/// validating every existing ancestor lexically and then opening the leaf with
/// `FILE_FLAG_OPEN_REPARSE_POINT`, which returns a handle to the reparse point
/// itself rather than its target. A redirected leaf is rejected by the
/// attribute check on the opened handle.
#[cfg(not(unix))]
pub(in crate::domain::collaboration_plugin) fn open_directory_path_no_follow(
    path: &Path,
) -> Result<fs::File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    ensure!(
        !absolute
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir)),
        "collaboration_plugin_destination_path_invalid"
    );
    licoup_foundation::platform::file_security::validate_no_symlink_ancestors(&absolute)?;
    let metadata = fs::symlink_metadata(&absolute)
        .map_err(|_| anyhow!("collaboration_plugin_destination_parent_invalid"))?;
    ensure!(
        metadata.is_dir(),
        "collaboration_plugin_destination_parent_invalid"
    );
    let directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&absolute)
        .map_err(|_| anyhow!("collaboration_plugin_destination_parent_invalid"))?;
    let opened = directory
        .metadata()
        .map_err(|_| anyhow!("collaboration_plugin_destination_parent_changed"))?;
    ensure!(
        opened.is_dir() && opened.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
        "collaboration_plugin_destination_parent_changed"
    );
    Ok(directory)
}

#[cfg(unix)]
fn open_canonical_directory_no_follow(path: &Path) -> Result<fs::File> {
    let mut current = open_directory(Path::new("/"))?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => current = open_existing_directory_at(&current, name)?,
            _ => return Err(anyhow!("collaboration_plugin_destination_path_invalid")),
        }
    }
    Ok(current)
}

#[cfg(unix)]
fn trusted_path_snapshots(path: &Path) -> Result<Vec<(PathBuf, u64, u64, bool)>> {
    use std::os::unix::fs::MetadataExt;

    let mut current = PathBuf::new();
    let mut snapshots = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::Normal(_) => current.push(component.as_os_str()),
            Component::CurDir => continue,
            Component::ParentDir | Component::Prefix(_) => {
                return Err(anyhow!("collaboration_plugin_destination_path_invalid"));
            }
        }
        let metadata = fs::symlink_metadata(&current)
            .map_err(|_| anyhow!("collaboration_plugin_destination_parent_invalid"))?;
        let symlink = metadata.file_type().is_symlink();
        ensure!(
            (symlink && metadata.uid() == 0) || (!symlink && metadata.is_dir()),
            "collaboration_plugin_destination_parent_invalid"
        );
        snapshots.push((current.clone(), metadata.dev(), metadata.ino(), symlink));
    }
    Ok(snapshots)
}

#[cfg(unix)]
fn open_directory(path: &Path) -> Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| anyhow!("collaboration_plugin_destination_parent_invalid"))
}

#[cfg(unix)]
fn component_cstring(name: &std::ffi::OsStr) -> Result<CString> {
    CString::new(name.as_bytes())
        .map_err(|_| anyhow!("collaboration_plugin_package_path_encoding_invalid"))
}

#[cfg(unix)]
fn open_directory_at(parent: &fs::File, name: &CString) -> Result<fs::File> {
    let flags =
        nix::libc::O_RDONLY | nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC;
    // SAFETY: the live directory descriptor and validated component remain valid
    // for `openat`; the result is checked before conversion.
    let fd = unsafe { nix::libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        return Err(if error.kind() == std::io::ErrorKind::NotFound {
            anyhow!("collaboration_plugin_package_directory_missing")
        } else {
            anyhow!("collaboration_plugin_package_directory_open_failed")
        });
    }
    // SAFETY: the successful `openat` returned a new descriptor owned here.
    Ok(unsafe { fs::File::from_raw_fd(fd) })
}

#[cfg(unix)]
fn open_existing_directory_at(parent: &fs::File, name: &std::ffi::OsStr) -> Result<fs::File> {
    open_directory_at(parent, &component_cstring(name)?)
}

#[cfg(unix)]
fn open_or_create_directory_at(parent: &fs::File, name: &std::ffi::OsStr) -> Result<fs::File> {
    let name = component_cstring(name)?;
    let flags =
        nix::libc::O_RDONLY | nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC;
    // SAFETY: the live directory descriptor and validated component remain valid
    // for `openat`; a negative return is handled below.
    let mut fd = unsafe { nix::libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        ensure!(
            error.kind() == std::io::ErrorKind::NotFound,
            "collaboration_plugin_package_directory_invalid"
        );
        // SAFETY: the directory descriptor and component remain live, and the
        // result is checked before the directory is opened again.
        let created = unsafe { nix::libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
        if created != 0 {
            let create_error = std::io::Error::last_os_error();
            ensure!(
                create_error.kind() == std::io::ErrorKind::AlreadyExists,
                "collaboration_plugin_package_directory_create_failed"
            );
        }
        // SAFETY: the same live descriptor/component pair is used after creation;
        // the result is checked immediately below.
        fd = unsafe { nix::libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    }
    ensure!(fd >= 0, "collaboration_plugin_package_directory_changed");
    // SAFETY: the successful `openat` returned a new descriptor owned here.
    Ok(unsafe { fs::File::from_raw_fd(fd) })
}

#[cfg(unix)]
fn open_or_create_relative_parent(root: &fs::File, relative: &Path) -> Result<fs::File> {
    let mut current = root.try_clone()?;
    let component_count = relative.components().count();
    for (index, component) in relative.components().enumerate() {
        let Component::Normal(name) = component else {
            return Err(anyhow!("collaboration_plugin_package_path_invalid"));
        };
        if index + 1 == component_count {
            break;
        }
        current = open_or_create_directory_at(&current, name)?;
    }
    Ok(current)
}

#[cfg(unix)]
fn open_existing_relative_parent(root: &fs::File, relative: &Path) -> Result<fs::File> {
    let mut current = root.try_clone()?;
    let component_count = relative.components().count();
    for (index, component) in relative.components().enumerate() {
        let Component::Normal(name) = component else {
            return Err(anyhow!("collaboration_plugin_package_path_invalid"));
        };
        if index + 1 == component_count {
            break;
        }
        current = open_existing_directory_at(&current, name)?;
    }
    Ok(current)
}

#[cfg(not(unix))]
fn create_directory_path_no_follow(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                current.push(component.as_os_str())
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                return Err(anyhow!("collaboration_plugin_package_path_invalid"));
            }
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) => ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "collaboration_plugin_package_directory_invalid"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current)?;
                licoup_foundation::platform::file_security::harden_private_path(&current)?;
                let metadata = fs::symlink_metadata(&current)?;
                ensure!(
                    metadata.is_dir() && !metadata.file_type().is_symlink(),
                    "collaboration_plugin_package_directory_changed"
                );
            }
            Err(_) => return Err(anyhow!("collaboration_plugin_package_directory_invalid")),
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}
