use super::*;
use licoup_foundation::core::acp;
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Install this package's own ACP dialect before any reducer is built.
///
/// The shared transport reads one Agent's frame dialect through the port the
/// composition above it answers, and a unit-test binary is not that composition:
/// `licoup-agent-drivers` installs a neutral dialect for a build that installed
/// nothing, so a suite that reaches a reducer first asserts against a stub. These
/// suites are the package's own, so they install the package's registration
/// first and the permission summary, the transition vocabulary and the
/// client-request answer they read are the ones a running Hermes produces.
///
/// It is idempotent, and it is called before every construction: the port's own
/// installation is first-wins, so a test that built a protocol before asking for
/// the package's dialect would leave the whole binary on the neutral one.
pub(super) fn install_package_dialect() {
    static INSTALLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    INSTALLED.get_or_init(|| {
        licoup_agent_drivers::acp_driver_runtime::parser_port::install(vec![
            crate::dialect::registration(),
        ])
    });
}

pub(super) fn absolute_test_cwd() -> PathBuf {
    #[cfg(target_os = "windows")]
    let root = PathBuf::from(format!("C:{}", std::path::MAIN_SEPARATOR));
    #[cfg(not(target_os = "windows"))]
    let root = PathBuf::from(std::path::MAIN_SEPARATOR.to_string());
    root.join("workspace").join("project")
}

pub(super) fn config(params: Value, prompt: &str, session_id: &str) -> ProtocolConfig {
    install_package_dialect();
    let cwd = absolute_test_cwd();
    ProtocolConfig::from_params(&params, prompt, session_id, Some(cwd.as_path())).unwrap()
}

pub(super) fn sent_messages(effects: Vec<ProtocolEffect>) -> Vec<Value> {
    effects
        .into_iter()
        .filter_map(|effect| match effect {
            ProtocolEffect::Send(message) => Some(message),
            ProtocolEffect::Complete(_)
            | ProtocolEffect::Fail(_)
            | ProtocolEffect::AwaitExternalApproval { .. } => None,
        })
        .collect()
}

pub(super) fn initialize(protocol: &mut SessionProtocol) -> Vec<ProtocolEffect> {
    protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": INITIALIZE_REQUEST_ID,
        "result": {
            "protocolVersion": acp::PROTOCOL_VERSION,
            "agentCapabilities": {"loadSession": true, "sessionCapabilities": {"resume": {}}},
            "agentInfo": {"name": "hermes-agent", "version": "test"}
        }
    }))
}

#[cfg(unix)]
pub(super) fn unique_temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("lico-{label}-{}", Uuid::new_v4()));
    fs::create_dir_all(&path).unwrap();
    path
}

#[cfg(unix)]
pub(super) struct PortableDataDirGuard {
    previous: Option<PathBuf>,
}

#[cfg(unix)]
impl PortableDataDirGuard {
    pub(super) fn isolate_under(root: &Path) -> Self {
        let previous = licoup_foundation::platform::paths::set_portable_data_dir_override(Some(
            root.join("portable-data"),
        ));
        Self { previous }
    }
}

#[cfg(unix)]
impl Drop for PortableDataDirGuard {
    fn drop(&mut self) {
        licoup_foundation::platform::paths::set_portable_data_dir_override(self.previous.take());
    }
}

#[cfg(unix)]
pub(super) fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).unwrap();
}
