use licoup_foundation::platform::paths::set_portable_data_dir_override;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) struct TestRoot {
    path: PathBuf,
}

impl TestRoot {
    pub(super) fn new(name: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "licoup-client-state-{}-{}-{}",
            name,
            std::process::id(),
            stamp
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub(super) struct PortableDataDirOverrideGuard {
    previous: Option<PathBuf>,
}

impl PortableDataDirOverrideGuard {
    pub(super) fn set(path: PathBuf) -> Self {
        Self {
            previous: set_portable_data_dir_override(Some(path)),
        }
    }
}

impl Drop for PortableDataDirOverrideGuard {
    fn drop(&mut self) {
        set_portable_data_dir_override(self.previous.take());
    }
}
