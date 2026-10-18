use super::*;

pub(crate) fn execute_rpc_cli(
    args: Vec<String>,
    portable_data_dir: Option<PathBuf>,
) -> Result<licoup_native::ffi::commands::CliExecution> {
    let _portable_data_dir = PortableDataDirOverrideGuard::set(portable_data_dir);
    licoup_native::ffi::commands::execute_cli(args)
}

pub(crate) struct PortableDataDirOverrideGuard {
    previous: Option<PathBuf>,
}

impl PortableDataDirOverrideGuard {
    pub(crate) fn set(path: Option<PathBuf>) -> Self {
        let previous = licoup_foundation::platform::paths::set_portable_data_dir_override(path);
        Self { previous }
    }
}

impl Drop for PortableDataDirOverrideGuard {
    fn drop(&mut self) {
        licoup_foundation::platform::paths::set_portable_data_dir_override(self.previous.take());
    }
}
