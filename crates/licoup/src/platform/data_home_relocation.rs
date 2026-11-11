//! Native owner for moving the complete application-managed data home.

use anyhow::{Result, anyhow, bail};
use licoup_foundation::platform::{
    data_home_access::{
        DataHomeRelocationAdmission, acquire_data_home_relocation_admission,
        acquire_data_home_relocation_lease,
    },
    file_security as security,
    paths::{self, DataHomeSelection, DataHomeSource},
};
use serde_json::{Value, json};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use uuid::Uuid;

const PREVIOUS_ROOT_MARKER: &str = "client-state/data-home-previous-root";
const ROOT_FOLDER_NAME: &str = "LicoUp";

/// Relocate the selected root to a new `LicoUp` child of the chosen parent.
/// The source is never removed. The operation is exposed only by the dedicated
/// one-shot stdio bridge process, which does not hold a shared root lease.
pub fn relocate(params: &Value) -> Result<Value> {
    require_confirmation(params)?;
    let selected = paths::selected_data_home()?;
    ensure_user_selectable_root(&selected)?;
    let source_spelling = selected.path.clone();
    let source = source_spelling
        .canonicalize()
        .map_err(|_| anyhow!("data_home_source_unavailable"))?;
    if !source.is_dir() {
        bail!("data_home_source_unavailable");
    }

    let parent = params
        .get("destinationParent")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("data_home_destination_invalid"))?;
    if !parent.is_absolute() {
        bail!("data_home_destination_invalid");
    }
    let parent = parent
        .canonicalize()
        .map_err(|_| anyhow!("data_home_destination_invalid"))?;
    if !parent.is_dir() {
        bail!("data_home_destination_invalid");
    }
    let destination = parent.join(ROOT_FOLDER_NAME);
    reject_nested_roots(&source, &destination)?;
    ensure_path_absent(&destination)?;

    // Capture settings before stopping their processes. They are reinstalled
    // with the same enablement after the destination becomes authoritative.
    let autostart =
        crate::platform::client_autostart::status(&crate::target_port::agent_target_port())?;

    // Close admission before stopping root-local services. Their existing
    // leases remain valid until each writer proves it has drained; then the
    // exclusive access lease covers copy through locator publication.
    let admission = acquire_data_home_relocation_admission()?;
    stop_root_writers(&admission)?;
    let _relocation = admission.wait_for_process_access()?;
    let current = paths::selected_data_home()?;
    ensure_user_selectable_root(&current)?;
    if current.path != source_spelling
        || current.path.canonicalize().ok().as_deref() != Some(source.as_path())
    {
        bail!("data_home_selection_changed");
    }
    ensure_path_absent(&destination)?;

    let staging = parent.join(format!(".LicoUp-{}.staging", Uuid::new_v4().simple()));
    fs::create_dir(&staging).map_err(|_| anyhow!("data_home_copy_failed"))?;
    let mut staging_guard = StagingDirectory(Some(staging.clone()));

    phase("copying-data");
    copy_data_home_tree(&source, &staging)?;

    phase("publishing-data");
    publish_staged_directory(
        &staging,
        &destination,
        &parent,
        security::sync_directory,
        || {
            phase("updating-owned-references");
            if let Err(error) =
                crate::domain::conversation::snapshots::relocate_copied_data_home_references(
                    &destination,
                    &source_spelling,
                    &destination,
                )
            {
                remove_published_copy(&destination)?;
                return Err(error);
            }
            if let Err(error) = write_previous_root_marker(&destination, &source) {
                remove_published_copy(&destination)?;
                return Err(error);
            }

            phase("switching-data-home");
            if let Err(error) = paths::save_data_home(&destination) {
                // Atomic locator replacement can succeed before a later permission or
                // parent-sync check reports an error. Keep the published directory if
                // the locator may already refer to it; deleting it would turn a
                // durability warning into a broken selection.
                let source_remains_selected = paths::selected_data_home().is_ok_and(|active| {
                    active.path.canonicalize().ok().as_deref() == Some(source.as_path())
                });
                if source_remains_selected {
                    remove_published_copy(&destination)?;
                    return Err(error);
                }
                let _ = restore_autostart_settings(&autostart);
                bail!("data_home_relocation_recovery_required");
            }
            if let Err(error) = restore_autostart_settings(&autostart) {
                let restored = paths::save_data_home(&source_spelling);
                let autostart_restored = restore_autostart_settings(&autostart);
                if restored.is_err() || autostart_restored.is_err() {
                    // Keep the complete copy in place so the user has both roots to
                    // recover from if the operating-system registration also failed.
                    bail!("data_home_relocation_recovery_required");
                }
                remove_published_copy(&destination)?;
                return Err(error);
            }
            Ok(())
        },
    )?;
    staging_guard.0 = None;

    phase("complete");
    Ok(json!({
        "status": "relocated",
        "previousDataHome": source_spelling,
        "dataHome": destination,
    }))
}

/// Read the active selection and the optional recoverable source recorded by a
/// successful relocation. This is a read-only operation for the ordinary
/// process-lifetime RPC session.
pub fn status() -> Result<Value> {
    let selected = paths::selected_data_home()?;
    let previous = read_previous_root(&selected.path)?;
    let previous_cleanup_target = previous
        .as_deref()
        .and_then(|path| canonical_previous_root(&selected.path, path).ok());
    Ok(json!({
        "status": "available",
        "previousRootAvailable": previous_cleanup_target.is_some(),
        "previousRootPath": previous_cleanup_target
            .as_deref()
            .map(|path| path.to_string_lossy().into_owned()),
    }))
}

/// Change a missing saved selection to an already existing user-selected root.
/// This never creates an empty replacement directory and never copies or
/// removes data.
pub fn recover(params: &Value) -> Result<Value> {
    require_confirmation(params)?;
    let selected = paths::selected_data_home()?;
    if selected.source != DataHomeSource::Saved {
        bail!("data_home_recovery_not_available");
    }
    if selected.path.is_dir() {
        bail!("data_home_recovery_not_required");
    }
    let replacement = params
        .get("dataHome")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("data_home_destination_invalid"))?;
    if !replacement.is_absolute() {
        bail!("data_home_destination_invalid");
    }
    let replacement = replacement
        .canonicalize()
        .map_err(|_| anyhow!("data_home_destination_invalid"))?;
    if !replacement.is_dir() {
        bail!("data_home_destination_invalid");
    }
    // Root-local service endpoints cannot be addressed safely when the saved
    // volume is gone. The boot-locator coordinator is outside that root, so
    // taking its exclusive lease waits for every remaining LicoUp process
    // without recreating the missing directory.
    phase("waiting-for-native-access");
    let _relocation = acquire_data_home_relocation_lease()?;
    let current = paths::selected_data_home()?;
    if current.source != DataHomeSource::Saved
        || current.path != selected.path
        || current.path.is_dir()
        || !replacement.is_dir()
    {
        bail!("data_home_selection_changed");
    }
    phase("switching-data-home");
    paths::save_data_home(&replacement)?;
    if crate::platform::client_autostart::refresh_after_data_home_recovery(
        &crate::target_port::agent_target_port(),
    )
    .is_err()
    {
        bail!("data_home_recovery_autostart_failed");
    }
    Ok(json!({"status": "recovered"}))
}

/// Move the explicitly retained previous root to the operating-system trash.
/// The path comes only from the private marker written by this owner; the UI
/// cannot supply an arbitrary deletion target.
pub fn cleanup_previous(params: &Value) -> Result<Value> {
    require_confirmation(params)?;
    let selected = paths::selected_data_home()?;
    ensure_user_selectable_root(&selected)?;
    let current = selected
        .path
        .canonicalize()
        .map_err(|_| anyhow!("data_home_source_unavailable"))?;
    let marker = selected.path.join(PREVIOUS_ROOT_MARKER);
    let previous_selection = read_previous_root(&selected.path)?
        .ok_or_else(|| anyhow!("data_home_previous_root_unavailable"))?;
    let previous = canonical_previous_root(&current, &previous_selection)?;
    let expected_previous = params
        .get("expectedPreviousRootPath")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("data_home_previous_root_unavailable"))?;
    if expected_previous != previous {
        bail!("data_home_selection_changed");
    }

    let admission = acquire_data_home_relocation_admission()?;
    stop_root_writers(&admission)?;
    let _relocation = admission.wait_for_process_access()?;

    let latest = paths::selected_data_home()?;
    if latest.path != selected.path
        || latest.path.canonicalize().ok().as_deref() != Some(current.as_path())
        || read_previous_root(&latest.path)?.as_deref() != Some(previous_selection.as_path())
        || canonical_previous_root(&current, &previous_selection)
            .ok()
            .as_deref()
            != Some(previous.as_path())
    {
        bail!("data_home_selection_changed");
    }
    validate_previous_root(&current, &previous)?;
    phase("cleaning-previous-root");
    cleanup_previous_at(&current, &previous, &marker, |path| {
        trash::delete(path).map_err(|_| anyhow!("data_home_previous_root_cleanup_failed"))
    })?;
    Ok(json!({"status": "previous_root_trashed"}))
}

fn require_confirmation(params: &Value) -> Result<()> {
    if params.get("confirmed").and_then(Value::as_bool) != Some(true) {
        bail!("data_home_confirmation_required");
    }
    Ok(())
}

fn validate_previous_root(current: &Path, previous: &Path) -> Result<()> {
    if !current.is_dir() || !previous.is_dir() {
        bail!("data_home_previous_root_unavailable");
    }
    let current = current
        .canonicalize()
        .map_err(|_| anyhow!("data_home_source_unavailable"))?;
    let previous = previous
        .canonicalize()
        .map_err(|_| anyhow!("data_home_previous_root_unavailable"))?;
    reject_nested_roots(&current, &previous)
}

fn canonical_previous_root(current: &Path, selected: &Path) -> Result<PathBuf> {
    let previous = selected
        .canonicalize()
        .map_err(|_| anyhow!("data_home_previous_root_unavailable"))?;
    validate_previous_root(current, &previous)?;
    Ok(previous)
}

fn cleanup_previous_at(
    current: &Path,
    previous: &Path,
    marker: &Path,
    move_to_trash: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    validate_previous_root(current, previous)?;
    move_to_trash(previous)?;
    let removed = security::remove_private_state_marker(marker)
        .map_err(|_| anyhow!("data_home_previous_root_marker_cleanup_failed"))?;
    if !removed {
        bail!("data_home_previous_root_marker_cleanup_failed");
    }
    Ok(())
}

fn publish_staged_directory(
    staging: &Path,
    destination: &Path,
    parent: &Path,
    sync_parent: impl FnOnce(&Path) -> Result<()>,
    after_sync: impl FnOnce() -> Result<()>,
) -> Result<()> {
    rename_directory_without_replacing(staging, destination)
        .map_err(|_| anyhow!("data_home_destination_unavailable"))?;
    sync_parent(parent).map_err(|_| anyhow!("data_home_copy_failed"))?;
    after_sync()
}

fn read_previous_root(root: &Path) -> Result<Option<PathBuf>> {
    let marker = root.join(PREVIOUS_ROOT_MARKER);
    match fs::symlink_metadata(&marker) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => bail!("data_home_previous_root_unavailable"),
        Ok(_) => {}
    }
    let Some(contents) = security::read_existing_private_text_bounded(&marker, 32 * 1024)? else {
        return Ok(None);
    };
    let previous = PathBuf::from(contents.trim());
    if !previous.is_absolute() {
        bail!("data_home_previous_root_invalid");
    }
    Ok(Some(previous))
}

fn ensure_user_selectable_root(selection: &DataHomeSelection) -> Result<()> {
    match selection.source {
        DataHomeSource::Environment | DataHomeSource::LegacyEnvironment => {
            bail!("data_home_environment_selected")
        }
        DataHomeSource::Saved | DataHomeSource::Default | DataHomeSource::TestOverride => Ok(()),
    }
}

fn reject_nested_roots(source: &Path, destination: &Path) -> Result<()> {
    if destination == source || destination.starts_with(source) || source.starts_with(destination) {
        bail!("data_home_destination_nested");
    }
    Ok(())
}

fn ensure_path_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => bail!("data_home_destination_exists"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => bail!("data_home_destination_unavailable"),
    }
}

fn phase(name: &str) {
    eprintln!("LICOUP_DATA_HOME_PHASE={name}");
}

fn stop_root_writers(admission: &DataHomeRelocationAdmission) -> Result<()> {
    phase("stopping-conversation-host");
    crate::platform::conversation_host_client::stop_existing_and_wait()?;
    if admission.process_access_drained()? {
        phase("waiting-for-native-access");
        return Ok(());
    }
    phase("stopping-mcp-service");
    let mcp_stop = crate::platform::mcp_service_process::stop_for_data_home_transition();
    phase("stopping-gateway");
    crate::platform::gateway_runtime::service_stop_managed()?;
    // An optional service executable may be absent. Its control failure is
    // harmless only when the closed-admission OS lock proves no writer remains.
    if !admission.process_access_drained()? {
        mcp_stop?;
    }
    phase("waiting-for-native-access");
    Ok(())
}

struct StagingDirectory(Option<PathBuf>);

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn copy_data_home_tree(source: &Path, destination: &Path) -> Result<()> {
    let root_permissions = fs::symlink_metadata(source)
        .map_err(|_| anyhow!("data_home_copy_failed"))?
        .permissions();
    set_private_creation_permissions(destination)?;
    let mut stack = vec![DirectoryFrame::open(source, destination)?];
    while let Some(frame) = stack.last_mut() {
        let next = frame
            .entries
            .next()
            .transpose()
            .map_err(|_| anyhow!("data_home_copy_failed"))?;
        let Some(entry) = next else {
            let frame = stack.pop().expect("directory frame exists");
            let permissions = if frame.source == source {
                root_permissions.clone()
            } else {
                fs::symlink_metadata(&frame.source)
                    .map_err(|_| anyhow!("data_home_copy_failed"))?
                    .permissions()
            };
            fs::set_permissions(&frame.destination, permissions)
                .map_err(|_| anyhow!("data_home_copy_failed"))?;
            security::sync_directory(&frame.destination)
                .map_err(|_| anyhow!("data_home_copy_failed"))?;
            continue;
        };

        let from = entry.path();
        let to = frame.destination.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|_| anyhow!("data_home_copy_failed"))?;
        if file_type.is_dir() {
            fs::create_dir(&to).map_err(|_| anyhow!("data_home_copy_failed"))?;
            set_private_creation_permissions(&to)?;
            stack.push(DirectoryFrame::open(&from, &to)?);
        } else if file_type.is_file() {
            copy_regular_file(&from, &to)?;
        } else if file_type.is_symlink() {
            copy_symbolic_link(&from, &to)?;
        } else {
            bail!("data_home_copy_unsupported_entry");
        }
    }
    Ok(())
}

struct DirectoryFrame {
    source: PathBuf,
    destination: PathBuf,
    entries: fs::ReadDir,
}

impl DirectoryFrame {
    fn open(source: &Path, destination: &Path) -> Result<Self> {
        Ok(Self {
            source: source.to_path_buf(),
            destination: destination.to_path_buf(),
            entries: fs::read_dir(source).map_err(|_| anyhow!("data_home_copy_failed"))?,
        })
    }
}

#[cfg(unix)]
fn set_private_creation_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| anyhow!("data_home_copy_failed"))
}

#[cfg(not(unix))]
fn set_private_creation_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn copy_regular_file(source: &Path, destination: &Path) -> Result<()> {
    let mut input = fs::File::open(source).map_err(|_| anyhow!("data_home_copy_failed"))?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut output = options
        .open(destination)
        .map_err(|_| anyhow!("data_home_copy_failed"))?;
    io::copy(&mut input, &mut output).map_err(|_| anyhow!("data_home_copy_failed"))?;
    output
        .sync_all()
        .map_err(|_| anyhow!("data_home_copy_failed"))?;
    let permissions = fs::symlink_metadata(source)
        .map_err(|_| anyhow!("data_home_copy_failed"))?
        .permissions();
    fs::set_permissions(destination, permissions).map_err(|_| anyhow!("data_home_copy_failed"))?;
    output
        .sync_all()
        .map_err(|_| anyhow!("data_home_copy_failed"))?;
    Ok(())
}

#[cfg(unix)]
fn copy_symbolic_link(source: &Path, destination: &Path) -> Result<()> {
    use std::os::unix::fs::symlink;
    symlink(
        fs::read_link(source).map_err(|_| anyhow!("data_home_copy_failed"))?,
        destination,
    )
    .map_err(|_| anyhow!("data_home_copy_failed"))
}

#[cfg(windows)]
fn copy_symbolic_link(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::fs::{symlink_dir, symlink_file};
    let target = fs::read_link(source).map_err(|_| anyhow!("data_home_copy_failed"))?;
    let is_directory = fs::metadata(source).is_ok_and(|metadata| metadata.is_dir());
    if is_directory {
        symlink_dir(target, destination).map_err(|_| anyhow!("data_home_copy_failed"))
    } else {
        symlink_file(target, destination).map_err(|_| anyhow!("data_home_copy_failed"))
    }
}

#[cfg(not(any(unix, windows)))]
fn copy_symbolic_link(_source: &Path, _destination: &Path) -> Result<()> {
    bail!("data_home_copy_unsupported_entry")
}

#[cfg(target_os = "macos")]
fn rename_directory_without_replacing(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt as _;
    let source = std::ffi::CString::new(source.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let result =
        unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn rename_directory_without_replacing(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt as _;
    let source = std::ffi::CString::new(source.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "windows")]
fn rename_directory_without_replacing(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;

    let source: Vec<u16> = source.as_os_str().encode_wide().chain([0]).collect();
    let destination: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
    if unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileW(source.as_ptr(), destination.as_ptr())
    } != 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn rename_directory_without_replacing(_source: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "exclusive directory rename unavailable",
    ))
}

fn write_previous_root_marker(destination: &Path, previous: &Path) -> Result<()> {
    let marker = destination.join(PREVIOUS_ROOT_MARKER);
    let parent = marker
        .parent()
        .ok_or_else(|| anyhow!("data_home_copy_failed"))?;
    security::ensure_private_dir(parent)?;
    security::atomic_write_private_text_bounded(
        &marker,
        &format!("{}\n", previous.display()),
        32 * 1024,
    )?;
    Ok(())
}

fn remove_published_copy(destination: &Path) -> Result<()> {
    fs::remove_dir_all(destination).map_err(|_| anyhow!("data_home_recovery_cleanup_failed"))
}

fn restore_autostart_settings(status: &Value) -> Result<()> {
    if status.get("supported").and_then(Value::as_bool) != Some(true) {
        return Ok(());
    }
    let desktop = status
        .get("desktop")
        .ok_or_else(|| anyhow!("client_autostart_state_unavailable"))?;
    let mcp = status
        .get("mcp")
        .ok_or_else(|| anyhow!("client_autostart_state_unavailable"))?;
    let gateway = status
        .get("gateway")
        .ok_or_else(|| anyhow!("client_autostart_state_unavailable"))?;

    if desktop.get("installed").and_then(Value::as_bool) == Some(true)
        || desktop.get("enabled").and_then(Value::as_bool) == Some(true)
    {
        crate::platform::client_autostart::set_desktop(
            &crate::target_port::agent_target_port(),
            desktop.get("enabled").and_then(Value::as_bool) == Some(true),
            desktop.get("silent").and_then(Value::as_bool) == Some(true),
        )?;
    }
    if mcp.get("installed").and_then(Value::as_bool) == Some(true)
        || mcp.get("enabled").and_then(Value::as_bool) == Some(true)
    {
        crate::platform::client_autostart::set_mcp(
            &crate::target_port::agent_target_port(),
            mcp.get("enabled").and_then(Value::as_bool) == Some(true),
        )?;
    }
    if gateway.get("installed").and_then(Value::as_bool) == Some(true)
        || gateway.get("enabled").and_then(Value::as_bool) == Some(true)
    {
        let port = gateway
            .get("port")
            .and_then(Value::as_u64)
            .and_then(|port| u16::try_from(port).ok())
            .unwrap_or(15_722);
        crate::platform::client_autostart::set_gateway(
            &crate::target_port::agent_target_port(),
            gateway.get("enabled").and_then(Value::as_bool) == Some(true),
            port,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::{fs, path::Path};

    fn fixture(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "licoup-data-home-{name}-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    #[cfg(unix)]
    fn copy_preserves_sqlite_wal_commits_and_does_not_follow_external_links() {
        let parent = fixture("copy");
        let source = parent.join("source");
        let stage = parent.join("stage");
        let external = parent.join("external");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&stage).unwrap();
        fs::create_dir(&external).unwrap();
        fs::write(external.join("outside.txt"), b"outside").unwrap();
        symlink(&external, source.join("external-link")).unwrap();

        let database = source.join("conversations.sqlite3");
        let connection = Connection::open(&database).unwrap();
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        connection
            .execute_batch("CREATE TABLE records (id INTEGER PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        connection
            .execute(
                "INSERT INTO records(value) VALUES (?1)",
                ["committed in wal"],
            )
            .unwrap();
        assert!(Path::new(&format!("{}-wal", database.display())).is_file());

        copy_data_home_tree(&source, &stage).unwrap();
        assert_eq!(
            fs::read(stage.join("external-link/outside.txt")).unwrap(),
            b"outside"
        );
        assert!(
            fs::symlink_metadata(stage.join("external-link"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let copied = Connection::open(stage.join("conversations.sqlite3")).unwrap();
        let value: String = copied
            .query_row("SELECT value FROM records WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(value, "committed in wal");

        drop(copied);
        drop(connection);
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn failed_copy_keeps_source_and_discards_unpublished_stage() {
        use std::os::unix::ffi::OsStrExt as _;

        let parent = fixture("copy-failure");
        let source = parent.join("source");
        let stage = parent.join(".LicoUp-test.staging");
        let destination = parent.join(ROOT_FOLDER_NAME);
        fs::create_dir(&source).unwrap();
        fs::write(source.join("owned.txt"), b"still here").unwrap();
        let fifo = source.join("unsupported-pipe");
        let path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);

        fs::create_dir(&stage).unwrap();
        let staging_guard = StagingDirectory(Some(stage.clone()));
        assert!(copy_data_home_tree(&source, &stage).is_err());
        drop(staging_guard);

        assert_eq!(fs::read(source.join("owned.txt")).unwrap(), b"still here");
        assert!(fifo.exists());
        assert!(!stage.exists());
        assert!(!destination.exists());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn publication_sync_failure_prevents_the_selection_commit() {
        let parent = fixture("publish-sync-failure");
        let source = parent.join("source");
        let stage = parent.join("stage");
        let destination = parent.join(ROOT_FOLDER_NAME);
        fs::create_dir(&source).unwrap();
        fs::write(source.join("source.sqlite3"), b"source remains").unwrap();
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("source.sqlite3"), b"complete copy").unwrap();
        let mut selection_committed = false;

        let result = publish_staged_directory(
            &stage,
            &destination,
            &parent,
            |_| Err(anyhow!("synthetic directory sync failure")),
            || {
                selection_committed = true;
                Ok(())
            },
        );

        assert_eq!(result.unwrap_err().to_string(), "data_home_copy_failed");
        assert!(!selection_committed);
        assert_eq!(
            fs::read(source.join("source.sqlite3")).unwrap(),
            b"source remains"
        );
        assert_eq!(
            fs::read(destination.join("source.sqlite3")).unwrap(),
            b"complete copy"
        );
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn exclusive_publish_never_replaces_a_destination_created_after_preflight() {
        let parent = fixture("publish");
        let stage = parent.join("stage");
        let destination = parent.join(ROOT_FOLDER_NAME);
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("data"), b"copy").unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("keep"), b"existing").unwrap();

        assert!(rename_directory_without_replacing(&stage, &destination).is_err());
        assert_eq!(fs::read(destination.join("keep")).unwrap(), b"existing");
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn rejects_a_destination_nested_under_either_root() {
        assert!(reject_nested_roots(Path::new("/data/root"), Path::new("/data/root/new")).is_err());
        assert!(reject_nested_roots(Path::new("/data/root/new"), Path::new("/data/root")).is_err());
        assert!(reject_nested_roots(Path::new("/data/root"), Path::new("/other/LicoUp")).is_ok());
    }

    #[test]
    fn confirmed_previous_root_cleanup_removes_only_the_retained_fixture() {
        let parent = fixture("cleanup");
        let current = parent.join("current");
        let previous = parent.join("previous");
        fs::create_dir(&current).unwrap();
        fs::create_dir(&previous).unwrap();
        fs::write(current.join("current.sqlite3"), b"keep current").unwrap();
        fs::write(previous.join("previous.sqlite3"), b"retained source").unwrap();
        let marker = current.join("previous-root");
        security::ensure_private_dir(&current).unwrap();
        security::atomic_write_private_text_bounded(
            &marker,
            &format!("{}\n", previous.display()),
            32 * 1024,
        )
        .unwrap();

        cleanup_previous_at(&current, &previous, &marker, |path| {
            fs::remove_dir_all(path)?;
            Ok(())
        })
        .unwrap();

        assert!(current.join("current.sqlite3").is_file());
        assert!(!previous.exists());
        assert!(!marker.exists());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn marker_cleanup_failure_reports_that_the_previous_root_was_already_trashed() {
        let parent = fixture("cleanup-marker-failure");
        let current = parent.join("current");
        let previous = parent.join("previous");
        fs::create_dir(&current).unwrap();
        fs::create_dir(&previous).unwrap();
        fs::write(previous.join("retained.sqlite3"), b"retained source").unwrap();
        let marker = current.join("previous-root");
        fs::create_dir(&marker).unwrap();

        let result = cleanup_previous_at(&current, &previous, &marker, |path| {
            fs::remove_dir_all(path)?;
            Ok(())
        });

        assert_eq!(
            result.unwrap_err().to_string(),
            "data_home_previous_root_marker_cleanup_failed"
        );
        assert!(current.is_dir());
        assert!(!previous.exists());
        assert!(marker.is_dir());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn confirmed_cleanup_moves_the_canonical_previous_root_when_marker_uses_symlink_alias() {
        let parent = fixture("cleanup-symlink");
        let current = parent.join("current");
        let previous = parent.join("previous");
        let alias = parent.join("previous-alias");
        fs::create_dir(&current).unwrap();
        fs::create_dir(&previous).unwrap();
        fs::write(previous.join("retained.sqlite3"), b"retained source").unwrap();
        symlink(&previous, &alias).unwrap();
        write_previous_root_marker(&current, &alias).unwrap();
        let marker = current.join(PREVIOUS_ROOT_MARKER);

        let prior = paths::set_portable_data_dir_override(Some(current.clone()));
        let cleanup_target = status().unwrap()["previousRootPath"]
            .as_str()
            .unwrap()
            .to_owned();
        paths::set_portable_data_dir_override(prior);

        let selected = read_previous_root(&current).unwrap().unwrap();
        let canonical = canonical_previous_root(&current, &selected).unwrap();
        assert_eq!(canonical, previous.canonicalize().unwrap());
        cleanup_previous_at(&current, &canonical, &marker, |path| {
            assert_eq!(path, previous.canonicalize().unwrap());
            assert_eq!(path.to_string_lossy(), cleanup_target);
            fs::remove_dir_all(path)?;
            Ok(())
        })
        .unwrap();

        assert!(current.is_dir());
        assert!(!previous.exists());
        assert!(
            fs::symlink_metadata(&alias)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(!marker.exists());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn cleanup_rejects_a_previous_root_that_contains_the_current_root() {
        let parent = fixture("cleanup-nested");
        let previous = parent.join("previous");
        let current = previous.join("current");
        fs::create_dir_all(&current).unwrap();
        let marker = current.join("previous-root");

        let result = cleanup_previous_at(&current, &previous, &marker, |_| {
            panic!("must not move an ancestor of the active root")
        });

        assert!(result.is_err());
        assert!(current.is_dir());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn status_identifies_the_retained_root_that_cleanup_will_move() {
        let parent = fixture("status");
        let current = parent.join("current");
        let previous = parent.join("previous");
        fs::create_dir(&current).unwrap();
        fs::create_dir(&previous).unwrap();
        write_previous_root_marker(&current, &previous).unwrap();
        let prior = paths::set_portable_data_dir_override(Some(current.clone()));

        let result = status().unwrap();

        paths::set_portable_data_dir_override(prior);
        assert_eq!(result["previousRootAvailable"], true);
        assert_eq!(
            result["previousRootPath"],
            previous.canonicalize().unwrap().display().to_string()
        );
        fs::remove_dir_all(parent).unwrap();
    }
}
