//! Real fixtures for the isolation suite.
//!
//! Everything here is synthetic and local: temporary managed roots created by
//! the test, a shell extension process written from the embedded fixture, and
//! the real production carrier, host and record types. No user file, account,
//! credential, network service or existing process is touched, and every path a
//! test uses lives under its own temporary root.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use licoup_application::{ActivationMode, ApplicationFailure, CapabilityDescriptor, ContractRange};
use licoup_extension_contracts::profile::ProfileDeclaration;
use licoup_native::platform::extension_host::isolation::{
    IsolatedProcessCarrier, IsolationMode, IsolationPolicy, ResolvedProgram, ResourceLimits,
    StaticPrograms,
};
use licoup_native::platform::extension_host::{
    ActivationReceipt, CarrierSpec, ExtensionCarrier, ExtensionHost, InvocationBinding,
    InvocationOutcome,
};

/// The synthetic extension process, embedded so a test cannot drift from it.
pub const AGENT_SH: &str = include_str!("../fixtures/agent.sh");
/// The outbound-connection probe.
pub const NET_PROBE_PY: &str = include_str!("../fixtures/net_probe.py");
/// The adversarial descendant probe: it forks once, optionally leaves its
/// process group, holds the stdout pipe for a bounded window and always exits
/// by itself.
pub const GROUP_ESCAPE_PY: &str = include_str!("../fixtures/group_escape.py");
/// The fork-free filesystem confinement probe.
pub const ROOT_PROBE_PY: &str = include_str!("../fixtures/root_probe.py");

/// A temporary managed root and its synthetic fixtures.
///
/// `outside` is a sibling of the managed root, not a child: a path that only
/// looks outside while still living under the root would prove nothing about a
/// root escape.
pub struct Sandbox {
    root: PathBuf,
    fixtures: PathBuf,
    outside: PathBuf,
}

impl Sandbox {
    pub fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!(
            "licoup-v7-iso-{tag}-{}-{nanos:x}",
            std::process::id()
        ));
        let fixtures = root.join("fixtures");
        std::fs::create_dir_all(&fixtures).expect("sandbox fixtures");
        std::fs::create_dir_all(root.join("instances")).expect("sandbox instances");
        let outside = std::env::temp_dir().join(format!(
            "licoup-v7-iso-outside-{tag}-{}-{nanos:x}",
            std::process::id()
        ));
        std::fs::create_dir_all(&outside).expect("sandbox outside");
        let sandbox = Self {
            root,
            fixtures,
            outside,
        };
        sandbox.write_executable("agent.sh", AGENT_SH);
        sandbox.write_executable("net_probe.py", NET_PROBE_PY);
        sandbox.write_executable("group_escape.py", GROUP_ESCAPE_PY);
        sandbox.write_executable("root_probe.py", ROOT_PROBE_PY);
        sandbox
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn agent_script(&self) -> PathBuf {
        self.fixtures.join("agent.sh")
    }

    pub fn net_probe(&self) -> PathBuf {
        self.fixtures.join("net_probe.py")
    }

    pub fn group_escape_script(&self) -> PathBuf {
        self.fixtures.join("group_escape.py")
    }

    pub fn root_probe_script(&self) -> PathBuf {
        self.fixtures.join("root_probe.py")
    }

    pub fn fixture_dir(&self) -> &Path {
        &self.fixtures
    }

    /// A writable root for one synthetic instance.
    pub fn instance_root(&self, name: &str) -> PathBuf {
        let path = self.root.join("instances").join(name);
        std::fs::create_dir_all(&path).expect("instance root");
        path
    }

    /// A synthetic file outside the managed root and every declared read root.
    pub fn outside_file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.outside.join(name);
        std::fs::write(&path, contents).expect("outside file");
        path
    }

    /// A synthetic path outside the managed root and every declared root.
    pub fn outside_path(&self, name: &str) -> PathBuf {
        self.outside.join(name)
    }

    fn write_executable(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.fixtures.join(name);
        std::fs::write(&path, contents).expect("fixture write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("fixture mode");
        }
        path
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_dir_all(&self.outside);
    }
}

/// The shell extension process for one mode, rooted at one instance directory.
pub fn shell_program(
    sandbox: &Sandbox,
    mode: &str,
    marker: &str,
    instance_root: &Path,
) -> ResolvedProgram {
    ResolvedProgram::new("/bin/bash")
        .with_args([
            sandbox.agent_script().display().to_string(),
            mode.to_owned(),
            marker.to_owned(),
        ])
        .with_read_root(sandbox.fixture_dir())
        .with_write_root(instance_root)
}

pub fn programs(package: &str, version: &str, program: ResolvedProgram) -> StaticPrograms {
    let mut programs = StaticPrograms::new();
    programs.insert(package, version, program);
    programs
}

pub fn carrier(programs: StaticPrograms, policy: IsolationPolicy) -> Arc<IsolatedProcessCarrier> {
    IsolatedProcessCarrier::new(Arc::new(programs), policy).expect("carrier")
}

pub fn carrier_result(
    programs: StaticPrograms,
    policy: IsolationPolicy,
) -> Result<Arc<IsolatedProcessCarrier>, ApplicationFailure> {
    IsolatedProcessCarrier::new(Arc::new(programs), policy)
}

pub fn policy(mode: IsolationMode, root: &Path) -> IsolationPolicy {
    IsolationPolicy::new(mode, root)
        .with_shutdown_grace_ms(300)
        .with_limits(ResourceLimits {
            call_wall_ms: 2_000,
            ..Default::default()
        })
}

pub fn carrier_spec(package: &str, version: &str, instance: &str, generation: u64) -> CarrierSpec {
    CarrierSpec {
        package_id: package.to_owned(),
        package_version: version.to_owned(),
        instance_id: instance.to_owned(),
        generation,
        profiles: vec!["agent-execution".to_owned()],
    }
}

pub fn host_of(carrier: Arc<dyn ExtensionCarrier>) -> ExtensionHost {
    ExtensionHost::without_journal(carrier, contract_range())
}

pub fn contract_range() -> ContractRange {
    ContractRange {
        major: 1,
        minimum_minor: 0,
    }
}

pub fn stage_request(
    package: &str,
    capabilities: &[&str],
) -> licoup_native::platform::extension_host::StageRequest {
    let descriptor: CapabilityDescriptor = serde_json::from_value(serde_json::json!({
        "pluginId": package,
        "implementationVersion": "1.0.0",
        "supportedContractRange": {"major": 1, "minimumMinor": 0},
        "capabilities": capabilities,
    }))
    .expect("descriptor");
    licoup_native::platform::extension_host::StageRequest {
        descriptor,
        methods: licoup_extension_contracts::profile::DeclaredMethods::minimal_agent(),
        profiles: vec![
            ProfileDeclaration::new("agent-execution", 1)
                .with_capabilities(capabilities.iter().copied()),
        ],
        activation: ActivationMode::OnDemand,
        permission_scope: vec!["scope:local".to_owned()],
    }
}

pub fn activate(
    host: &ExtensionHost,
    package: &str,
    capabilities: &[&str],
) -> Result<ActivationReceipt, ApplicationFailure> {
    let staged = host.stage(stage_request(package, capabilities))?;
    let prepared = host.prepare(staged)?;
    host.activate(prepared)
}

/// Whether a real process with this pid is still present.
///
/// Signal 0 asks the kernel the same question `kill -0` does.
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

pub fn wait_for(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if predicate() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn read_text(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// The payload of a settled call, refusing the outcomes that are not one.
pub fn payload_of(outcome: InvocationOutcome) -> serde_json::Value {
    match outcome {
        InvocationOutcome::Finished { payload } => payload,
        InvocationOutcome::Natural(output) => serde_json::Value::String(output.text().to_owned()),
        InvocationOutcome::Unknown { code } => {
            panic!("the call settled unknown: {code}")
        }
        InvocationOutcome::Admitted => panic!("the call is still admitted"),
    }
}

/// Wait until a call faults, returning the classified failure.
///
/// "Not ready yet" is not an outcome; anything else ends the wait.
pub fn failure_of(
    host: &ExtensionHost,
    binding: &InvocationBinding,
    timeout: Duration,
) -> licoup_application::ApplicationFailure {
    let deadline = Instant::now() + timeout;
    loop {
        match host.result(binding) {
            Ok(outcome) => panic!("the call settled instead of faulting: {outcome:?}"),
            Err(failure) if failure.code == "extension_result_not_ready" => {
                assert!(
                    Instant::now() < deadline,
                    "the call neither settled nor faulted inside its window"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(failure) => return failure,
        }
    }
}

/// Follow one in-flight call to its settlement, the way a consumer does: the
/// refusal for "not ready yet" is not an outcome.
pub fn settle(
    host: &ExtensionHost,
    binding: &InvocationBinding,
    timeout: Duration,
) -> InvocationOutcome {
    let deadline = Instant::now() + timeout;
    loop {
        match host.result(binding) {
            Ok(outcome) => return outcome,
            Err(failure) if failure.code == "extension_result_not_ready" => {
                assert!(
                    Instant::now() < deadline,
                    "the invocation did not settle inside its observation window"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(failure) => panic!(
                "the original owner could not settle its call: {} at {}",
                failure.code, failure.stage
            ),
        }
    }
}

/// Required test runtime, not a bundled product dependency. An unavailable
/// interpreter is a failing, explicitly unverifiable acceptance run, not a
/// successful early return. The override is test-only and never changes a
/// production carrier's environment or permissions.
pub struct PythonRuntime {
    pub executable: PathBuf,
    pub runtime_root: PathBuf,
    pub framework_app: Option<PathBuf>,
}

pub fn python_runtime() -> PythonRuntime {
    let executable =
        std::env::var_os("LICOUP_ISOLATION_TEST_PYTHON").unwrap_or_else(|| "python3".into());
    let runtime = probe_python(&executable).expect(
        "UNVERIFIABLE: required Python runtime is unavailable or invalid; no OS validation pass",
    );
    eprintln!("X2_RUNTIME python=executed discovery=isolated");
    runtime
}

fn probe_python(executable: &std::ffi::OsStr) -> Option<PythonRuntime> {
    let probe = "import os,sys\nprint(os.path.realpath(sys.executable))\nprint(sys.prefix)\nprint(os.path.join(sys.prefix,'Resources','Python.app','Contents','MacOS','Python'))\nprint(sys.version.split()[0])";
    let output = Command::new(executable)
        .args(["-I", "-B", "-c", probe])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let mut lines = text.lines();
    let executable = PathBuf::from(lines.next()?.trim());
    let runtime_root = PathBuf::from(lines.next()?.trim());
    let app = PathBuf::from(lines.next()?.trim());
    let version = lines.next()?.trim();
    if !executable.is_file() || !runtime_root.is_dir() {
        return None;
    }
    use sha2::Digest;
    let hash = sha2::Sha256::digest(std::fs::read(&executable).ok()?);
    eprintln!("X2_RUNTIME python-version={version} executable-sha256={hash:x}");
    Some(PythonRuntime {
        executable,
        runtime_root,
        framework_app: app.is_file().then_some(app),
    })
}

#[test]
fn missing_python_makes_the_real_acceptance_case_fail() {
    let sandbox = Sandbox::new("missing-runtime");
    let output = Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "a19_process_isolation::the_published_reference_sdk_sample_is_served_over_a_real_pipe",
            "--nocapture",
        ])
        .env(
            "LICOUP_ISOLATION_TEST_PYTHON",
            sandbox.root().join("absent-interpreter"),
        )
        .output()
        .expect("negative runtime acceptance invocation");
    assert!(
        !output.status.success(),
        "missing Python must never be a green acceptance run"
    );
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.contains("UNVERIFIABLE: required Python runtime"));
    assert!(output.contains("1 failed"));
    eprintln!("X2_PROBE runtime-missing acceptance=failed-as-required");
}

/// A real interpreter program with the runtime roots it needs, so the same
/// helper works for trusted-local and restricted runs.
pub fn python_program(
    python: &PythonRuntime,
    args: Vec<String>,
    sandbox: &Sandbox,
    instance_root: &Path,
) -> ResolvedProgram {
    let mut program = ResolvedProgram::new(python.executable.clone())
        .with_args(args)
        .with_read_root(sandbox.fixture_dir())
        .with_read_root(&python.runtime_root)
        .with_read_root("/usr/lib")
        .with_read_root("/System")
        .with_write_root(instance_root)
        .with_env("PYTHONDONTWRITEBYTECODE", "1");
    if let Some(app) = &python.framework_app {
        program = program.with_exec_path(app);
    }
    program
}

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn reference_sample() -> PathBuf {
    repo_root().join("sdk/agent-adapter/samples/minimal-specialist/agent.py")
}

pub fn reference_sdk_dir() -> PathBuf {
    repo_root().join("sdk/agent-adapter/python")
}
