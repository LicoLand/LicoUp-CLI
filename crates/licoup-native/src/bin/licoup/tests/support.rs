use super::*;

pub(super) fn execute_cli(
    args: Vec<String>,
) -> anyhow::Result<licoup_native::ffi::commands::CliExecution> {
    licoup_native::ffi::commands::execute_cli(args)
}

pub(super) fn set_portable_dir(path: &Path) -> PortableDirGuard {
    PortableDirGuard::set(path)
}

pub(super) fn json_payload(result: &licoup_native::ffi::commands::CliExecution) -> &Value {
    match result {
        licoup_native::ffi::commands::CliExecution::Json(value) => value,
        licoup_native::ffi::commands::CliExecution::Usage => {
            panic!("expected json result")
        }
        licoup_native::ffi::commands::CliExecution::Streamed => {
            panic!("expected json result")
        }
    }
}

pub(super) fn temp_cli_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let dir = env::temp_dir().join(format!(
        "licoup-native-test-{}-{}-{}",
        name,
        now.as_secs(),
        now.subsec_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

pub(super) fn cli_env_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

pub(super) struct PortableDirGuard {
    previous: Option<PathBuf>,
    #[cfg(target_os = "macos")]
    previous_macos_test_user_presence_disabled: bool,
}

impl PortableDirGuard {
    pub(super) fn set(path: &Path) -> Self {
        let previous = licoup_foundation::platform::paths::set_portable_data_dir_override(Some(
            path.to_path_buf(),
        ));
        #[cfg(target_os = "macos")]
            let previous_macos_test_user_presence_disabled =
                licoup_native::platform::secure_mesh_secret_store::set_macos_test_user_presence_disabled(true);
        Self {
            previous,
            #[cfg(target_os = "macos")]
            previous_macos_test_user_presence_disabled,
        }
    }
}

impl Drop for PortableDirGuard {
    fn drop(&mut self) {
        licoup_foundation::platform::paths::set_portable_data_dir_override(self.previous.take());
        #[cfg(target_os = "macos")]
        licoup_native::platform::secure_mesh_secret_store::set_macos_test_user_presence_disabled(
            self.previous_macos_test_user_presence_disabled,
        );
    }
}
