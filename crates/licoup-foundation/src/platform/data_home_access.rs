//! Process-lifetime access and relocation exclusion for the selected data home.
//!
//! Coordination files live beside the boot locator, never under the movable
//! root. Ordinary data-owning processes retain a shared lease until exit. A
//! relocation process first closes the admission barrier, then waits for all
//! shared leases before it copies any data.

use anyhow::{Result, anyhow, bail};
use fs2::FileExt;
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

const ADMISSION_LOCK: &str = "data-home-admission.lock";
const ACCESS_LOCK: &str = "data-home-access.lock";

static PROCESS_ACCESS_LEASES: AtomicUsize = AtomicUsize::new(0);

/// Whether a failed non-blocking lock attempt means another holder owns the lease.
/// `fs2` defines the native error on every supported platform; comparing that value
/// keeps contention distinct from permissions and other I/O failures.
fn lock_is_contended(error: &io::Error) -> bool {
    let expected = fs2::lock_contended_error();
    error.raw_os_error().is_some() && error.raw_os_error() == expected.raw_os_error()
}

/// A shared process lease. Keep this value alive for the lifetime of every
/// process that can read or write application-owned state.
pub struct ProcessDataHomeAccess {
    lease: Option<DataHomeAccessLease>,
}

impl Drop for ProcessDataHomeAccess {
    fn drop(&mut self) {
        self.lease.take();
        PROCESS_ACCESS_LEASES.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The exclusive authority held while the selected root is copied and saved.
pub struct DataHomeRelocationLease {
    _admission: File,
    _access: File,
}

/// Admission barrier held while owned services stop. Existing process leases
/// may drain, but no new root users can start before the exclusive access
/// lease is acquired.
pub struct DataHomeRelocationAdmission {
    _admission: File,
    access_path: PathBuf,
}

impl DataHomeRelocationAdmission {
    /// Prove that existing participants have drained while admission remains
    /// closed. The barrier prevents a new reader from racing this observation.
    pub fn process_access_drained(&self) -> Result<bool> {
        let access = open_lock(&self.access_path)?;
        match FileExt::try_lock_exclusive(&access) {
            Ok(()) => Ok(true),
            Err(error) if lock_is_contended(&error) => Ok(false),
            Err(_) => Err(anyhow!("data-home relocation lease failed")),
        }
    }

    pub fn wait_for_process_access(self) -> Result<DataHomeRelocationLease> {
        let access = open_lock(&self.access_path)?;
        FileExt::lock_exclusive(&access)
            .map_err(|_| anyhow!("data-home relocation lease failed"))?;
        Ok(DataHomeRelocationLease {
            _admission: self._admission,
            _access: access,
        })
    }
}

struct DataHomeAccessLease {
    _access: File,
}

/// Acquire the shared process lease from the current per-user locator area.
pub fn acquire_process_data_home_access() -> Result<ProcessDataHomeAccess> {
    let locator = locator_path()?;
    acquire_process_data_home_access_at(&locator)
}

fn acquire_process_data_home_access_at(locator: &Path) -> Result<ProcessDataHomeAccess> {
    PROCESS_ACCESS_LEASES.fetch_add(1, Ordering::AcqRel);
    match acquire_data_home_access_at(locator) {
        Ok(lease) => Ok(ProcessDataHomeAccess { lease: Some(lease) }),
        Err(error) => {
            PROCESS_ACCESS_LEASES.fetch_sub(1, Ordering::AcqRel);
            Err(error)
        }
    }
}

/// Close new-access admission and wait for every other process lease to end.
///
/// This operation must run from a dedicated process which has not acquired a
/// shared process lease. In particular, a persistent RPC process must be shut
/// down before invoking relocation; returning an error here prevents a
/// same-process self-deadlock.
pub fn acquire_data_home_relocation_lease() -> Result<DataHomeRelocationLease> {
    acquire_data_home_relocation_admission()?.wait_for_process_access()
}

/// Try to acquire the exclusive relocation lease without waiting for writers.
///
/// `Ok(None)` means another process holds the admission barrier or a shared access
/// lease: a capture or import must be refused rather than read a live root, and no
/// process is stopped by asking. Like the blocking form, this runs only in a process
/// that holds no shared lease.
pub fn try_acquire_data_home_relocation_lease() -> Result<Option<DataHomeRelocationLease>> {
    if PROCESS_ACCESS_LEASES.load(Ordering::Acquire) != 0 {
        bail!("data_home_relocation_requires_stopped_native_process");
    }
    try_acquire_data_home_relocation_lease_at(&locator_path()?)
}

/// Close process admission before stopping services. The returned barrier
/// must remain alive through service shutdown and access draining.
pub fn acquire_data_home_relocation_admission() -> Result<DataHomeRelocationAdmission> {
    if PROCESS_ACCESS_LEASES.load(Ordering::Acquire) != 0 {
        bail!("data_home_relocation_requires_stopped_native_process");
    }
    acquire_data_home_relocation_admission_at(&locator_path()?)
}

fn locator_path() -> Result<PathBuf> {
    super::paths::data_home_locator_path()
}

fn coordination_paths(locator: &Path) -> Result<(PathBuf, PathBuf)> {
    let parent = locator
        .parent()
        .ok_or_else(|| anyhow!("data-home coordination directory is unavailable"))?;
    Ok((parent.join(ADMISSION_LOCK), parent.join(ACCESS_LOCK)))
}

fn acquire_data_home_access_at(locator: &Path) -> Result<DataHomeAccessLease> {
    let (admission_path, access_path) = prepare_coordination_files(locator)?;
    let admission = open_lock(&admission_path)?;
    // Readers must not queue behind an active relocation barrier. A process
    // admitted after the barrier releases could still carry an old inherited
    // LICOUP_HOME and write the source after the locator has switched.
    match FileExt::try_lock_shared(&admission) {
        Ok(()) => {}
        Err(error) if lock_is_contended(&error) => {
            bail!("data-home relocation is in progress")
        }
        Err(_) => bail!("data-home access admission failed"),
    }
    let access = open_lock(&access_path)?;
    FileExt::lock_shared(&access).map_err(|_| anyhow!("data-home access lease failed"))?;
    FileExt::unlock(&admission).map_err(|_| anyhow!("data-home access admission failed"))?;
    Ok(DataHomeAccessLease { _access: access })
}

#[cfg(test)]
fn acquire_data_home_relocation_lease_at_with_admission(
    locator: &Path,
    admission_acquired: impl FnOnce(),
) -> Result<DataHomeRelocationLease> {
    let admission = acquire_data_home_relocation_admission_at(locator)?;
    admission_acquired();
    admission.wait_for_process_access()
}

fn acquire_data_home_relocation_admission_at(
    locator: &Path,
) -> Result<DataHomeRelocationAdmission> {
    let (admission_path, access_path) = prepare_coordination_files(locator)?;
    let admission = open_lock(&admission_path)?;
    FileExt::lock_exclusive(&admission)
        .map_err(|_| anyhow!("data-home relocation admission failed"))?;
    Ok(DataHomeRelocationAdmission {
        _admission: admission,
        access_path,
    })
}

#[cfg(test)]
fn try_acquire_data_home_access_at(locator: &Path) -> Result<Option<DataHomeAccessLease>> {
    let (admission_path, access_path) = prepare_coordination_files(locator)?;
    let admission = open_lock(&admission_path)?;
    match FileExt::try_lock_shared(&admission) {
        Ok(()) => {}
        Err(error) if lock_is_contended(&error) => return Ok(None),
        Err(_) => return Err(anyhow!("data-home access admission failed")),
    }
    let access = open_lock(&access_path)?;
    match FileExt::try_lock_shared(&access) {
        Ok(()) => {}
        Err(error) if lock_is_contended(&error) => return Ok(None),
        Err(_) => return Err(anyhow!("data-home access lease failed")),
    }
    FileExt::unlock(&admission).map_err(|_| anyhow!("data-home access admission failed"))?;
    Ok(Some(DataHomeAccessLease { _access: access }))
}

fn try_acquire_data_home_relocation_lease_at(
    locator: &Path,
) -> Result<Option<DataHomeRelocationLease>> {
    let (admission_path, access_path) = prepare_coordination_files(locator)?;
    let admission = open_lock(&admission_path)?;
    match FileExt::try_lock_exclusive(&admission) {
        Ok(()) => {}
        Err(error) if lock_is_contended(&error) => return Ok(None),
        Err(_) => return Err(anyhow!("data-home relocation admission failed")),
    }
    let access = open_lock(&access_path)?;
    match FileExt::try_lock_exclusive(&access) {
        Ok(()) => {}
        Err(error) if lock_is_contended(&error) => return Ok(None),
        Err(_) => return Err(anyhow!("data-home relocation lease failed")),
    }
    Ok(Some(DataHomeRelocationLease {
        _admission: admission,
        _access: access,
    }))
}

fn prepare_coordination_files(locator: &Path) -> Result<(PathBuf, PathBuf)> {
    let (admission, access) = coordination_paths(locator)?;
    let parent = locator
        .parent()
        .ok_or_else(|| anyhow!("data-home coordination directory is unavailable"))?;
    super::file_security::ensure_private_dir(parent)?;
    Ok((admission, access))
}

fn open_lock(path: &Path) -> Result<File> {
    super::file_security::open_private_lock_file(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead as _, BufReader, Write as _},
        process::{Command, Stdio},
        sync::mpsc,
        thread,
    };

    const HELPER_ACTION: &str = "LICOUP_TEST_DATA_HOME_LEASE_ACTION";
    const HELPER_LOCATOR: &str = "LICOUP_TEST_DATA_HOME_LEASE_LOCATOR";

    #[test]
    fn native_lock_contention_is_distinct_from_other_io_failures() {
        assert!(lock_is_contended(&fs2::lock_contended_error()));
        assert!(!lock_is_contended(&std::io::Error::from(
            std::io::ErrorKind::PermissionDenied
        )));
    }

    #[test]
    fn separate_process_lease_blocks_copy_until_released_and_admission_blocks_new_access() {
        let fixture = std::env::temp_dir().join(format!(
            "licoup-data-home-coordination-{}",
            uuid::Uuid::new_v4()
        ));
        let locator = fixture.join("config/data-home");
        std::fs::create_dir_all(locator.parent().unwrap()).unwrap();

        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "platform::data_home_access::tests::cross_process_lease_helper",
                "--nocapture",
            ])
            .env(HELPER_ACTION, "hold")
            .env(HELPER_LOCATOR, &locator)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            let read = stdout.read_line(&mut line).unwrap();
            assert_ne!(read, 0, "lease helper exited before acquiring its lease");
            if line.trim() == "DATA_HOME_LEASE_READY" {
                break;
            }
        }

        assert!(
            try_acquire_data_home_relocation_lease_at(&locator)
                .unwrap()
                .is_none()
        );

        let (admission_tx, admission_rx) = mpsc::channel();
        let worker_locator = locator.clone();
        let relocation = thread::spawn(move || {
            acquire_data_home_relocation_lease_at_with_admission(&worker_locator, || {
                admission_tx.send(()).unwrap();
            })
            .unwrap()
        });
        admission_rx.recv().unwrap();
        assert!(try_acquire_data_home_access_at(&locator).unwrap().is_none());

        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"release\n")
            .unwrap();
        child.stdin.as_mut().unwrap().flush().unwrap();
        assert!(child.wait().unwrap().success());
        let relocation = relocation.join().unwrap();

        assert!(try_acquire_data_home_access_at(&locator).unwrap().is_none());
        assert!(acquire_data_home_access_at(&locator).is_err());
        drop(relocation);
        assert!(try_acquire_data_home_access_at(&locator).unwrap().is_some());
        let _ = std::fs::remove_dir_all(fixture);
    }

    #[test]
    fn non_blocking_lease_refuses_an_active_writer_and_recovers_after_release() {
        let fixture = std::env::temp_dir().join(format!(
            "licoup-data-home-try-lease-{}",
            uuid::Uuid::new_v4()
        ));
        let locator = fixture.join("config/data-home");
        std::fs::create_dir_all(locator.parent().unwrap()).unwrap();

        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "platform::data_home_access::tests::cross_process_lease_helper",
                "--nocapture",
            ])
            .env(HELPER_ACTION, "hold")
            .env(HELPER_LOCATOR, &locator)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            let read = stdout.read_line(&mut line).unwrap();
            assert_ne!(read, 0, "lease helper exited before acquiring its lease");
            if line.trim() == "DATA_HOME_LEASE_READY" {
                break;
            }
        }

        // An active writer is refused without being stopped or signalled.
        assert!(
            try_acquire_data_home_relocation_lease_at(&locator)
                .unwrap()
                .is_none()
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "the writer stays alive"
        );

        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"release\n")
            .unwrap();
        child.stdin.as_mut().unwrap().flush().unwrap();
        assert!(child.wait().unwrap().success());

        let lease = try_acquire_data_home_relocation_lease_at(&locator)
            .unwrap()
            .expect("the lease is available once the writer drained");
        assert!(try_acquire_data_home_access_at(&locator).unwrap().is_none());
        drop(lease);
        assert!(try_acquire_data_home_access_at(&locator).unwrap().is_some());
        let _ = std::fs::remove_dir_all(fixture);
    }

    #[test]
    fn admission_barrier_can_precede_service_stop_and_wait_for_existing_access() {
        let fixture = std::env::temp_dir().join(format!(
            "licoup-data-home-admission-{}",
            uuid::Uuid::new_v4()
        ));
        let locator = fixture.join("config/data-home");
        std::fs::create_dir_all(locator.parent().unwrap()).unwrap();

        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "platform::data_home_access::tests::cross_process_lease_helper",
                "--nocapture",
            ])
            .env(HELPER_ACTION, "hold")
            .env(HELPER_LOCATOR, &locator)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            let read = stdout.read_line(&mut line).unwrap();
            assert_ne!(read, 0, "lease helper exited before acquiring its lease");
            if line.trim() == "DATA_HOME_LEASE_READY" {
                break;
            }
        }

        let admission = acquire_data_home_relocation_admission_at(&locator).unwrap();
        assert!(try_acquire_data_home_access_at(&locator).unwrap().is_none());
        assert!(!admission.process_access_drained().unwrap());

        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"release\n")
            .unwrap();
        child.stdin.as_mut().unwrap().flush().unwrap();
        assert!(child.wait().unwrap().success());
        assert!(admission.process_access_drained().unwrap());
        let relocation = admission.wait_for_process_access().unwrap();

        assert!(try_acquire_data_home_access_at(&locator).unwrap().is_none());
        drop(relocation);
        assert!(try_acquire_data_home_access_at(&locator).unwrap().is_some());
        let _ = std::fs::remove_dir_all(fixture);
    }

    #[test]
    fn cross_process_lease_helper() {
        let Ok(action) = std::env::var(HELPER_ACTION) else {
            return;
        };
        let locator = PathBuf::from(std::env::var_os(HELPER_LOCATOR).unwrap());
        if action == "self-check" {
            let _lease = acquire_process_data_home_access_at(&locator).unwrap();
            assert!(acquire_data_home_relocation_lease().is_err());
            return;
        }
        assert_eq!(action, "hold");
        let _lease = acquire_data_home_access_at(&locator).unwrap();
        println!("DATA_HOME_LEASE_READY");
        std::io::stdout().flush().unwrap();
        let mut release = String::new();
        BufReader::new(std::io::stdin())
            .read_line(&mut release)
            .unwrap();
        assert_eq!(release.trim(), "release");
    }

    #[test]
    fn relocation_process_rejects_its_own_shared_lease_without_waiting() {
        let fixture = std::env::temp_dir().join(format!(
            "licoup-data-home-self-lock-{}",
            uuid::Uuid::new_v4()
        ));
        let locator = fixture.join("config/data-home");
        std::fs::create_dir_all(locator.parent().unwrap()).unwrap();
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "platform::data_home_access::tests::cross_process_lease_helper",
                "--nocapture",
            ])
            .env(HELPER_ACTION, "self-check")
            .env(HELPER_LOCATOR, &locator)
            .status()
            .unwrap();
        assert!(result.success());
        let _ = std::fs::remove_dir_all(fixture);
    }
}
