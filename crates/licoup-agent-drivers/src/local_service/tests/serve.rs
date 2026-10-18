use super::super::endpoint::{ServeModel, ServeModelCatalog, ServeReadiness};
use super::super::process::{self, ServeRunner, SpawnFailure};
use super::super::state::{self, ServicePaths};
use super::super::{executable::ResolvedExecutable, serve, turn_control};
use anyhow::Result;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[test]
fn owned_generation_identity_requires_the_same_file_and_health_version() {
    let executable = ResolvedExecutable {
        path: "/synthetic/bin/agent".to_string(),
        private_file_identity: "private-file-id".to_string(),
    };
    let state = json!({
        "launchIdentity": {
            "file": "private-file-id",
            "version": "1.2.3"
        }
    });
    assert!(serve::launch_identity_matches_for_test(
        &state,
        &executable,
        "1.2.3"
    ));
    assert!(!serve::launch_identity_matches_for_test(
        &state,
        &executable,
        "1.2.4"
    ));
}

const READY_PROVIDERS: &str = r#"{"default":{"synthetic-provider":"current-model"}}"#;

fn synthetic_spec(default_port: u16) -> serve::ServeSpec {
    serve::ServeSpec {
        identity: "synthetic-local-service",
        default_port,
        port_range_span: 64,
        default_host: "127.0.0.1",
        health_path: "/global/health",
        session_probe_path: "/session",
        config_path: "/config",
        provider_path: "/provider",
        state_dir: "synthetic-local-service-test",
        state_schema_version: "synthetic-v1",
        default_health_timeout_ms: 5_000,
        reserved_ports: &[],
        executable_environment: &[],
        default_executable: "synthetic-missing-executable",
        configure_command: noop_configure,
        parse_readiness: synthetic_readiness,
        errors: serve::ServeErrorCodes {
            executable_missing: "synthetic_executable_missing",
            port_exhausted: "synthetic_port_exhausted",
            start_failed: "synthetic_start_failed",
            health_failed: "synthetic_health_failed",
            attach_probe_failed: "synthetic_attach_probe_failed",
            not_found: "synthetic_not_found",
            request_failed: "synthetic_request_failed",
            invalid_json: "synthetic_invalid_json",
            invalid_state: "synthetic_invalid_state",
            stop_failed: "synthetic_stop_failed",
        },
    }
}

fn noop_configure(_command: &mut Command, _host: &str, _port: u16) {}

fn synthetic_readiness(
    health: &Value,
    sessions: &Value,
    _config: &Value,
    providers: &Value,
) -> Option<ServeReadiness> {
    if health.get("healthy").and_then(Value::as_bool) != Some(true) || !sessions.is_array() {
        return None;
    }
    let version = health.get("version")?.as_str()?.trim();
    if version.is_empty() {
        return None;
    }
    let defaults = providers.get("default")?.as_object()?;
    let (provider_id, model_id) = defaults.iter().next()?;
    let provider_id = provider_id.trim();
    let model_id = model_id.as_str()?.trim();
    if provider_id.is_empty() || model_id.is_empty() {
        return None;
    }
    let current = ServeModel {
        provider_id: provider_id.to_string(),
        model_id: model_id.to_string(),
    };
    Some(ServeReadiness {
        version: version.to_string(),
        catalog: ServeModelCatalog {
            current: current.clone(),
            models: vec![current],
        },
        health: json!({"healthy": true, "version": version}),
    })
}

enum ProviderPlan {
    Fixed(String),
    FlipAfter {
        failures: AtomicUsize,
        ready: String,
    },
}

impl ProviderPlan {
    fn body(&self) -> String {
        match self {
            Self::Fixed(body) => body.clone(),
            Self::FlipAfter { failures, ready } => {
                let remaining = failures.load(Ordering::SeqCst);
                if remaining > 0 {
                    failures.store(remaining - 1, Ordering::SeqCst);
                    "{}".to_string()
                } else {
                    ready.clone()
                }
            }
        }
    }
}

fn read_request_head(stream: &mut TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        if stream.read(&mut byte).map(|read| read == 0).unwrap_or(true) {
            return None;
        }
        head.push(byte[0]);
        if head.len() > 16 * 1024 {
            return None;
        }
    }
    String::from_utf8_lossy(&head)
        .split("\r\n")
        .next()
        .map(str::to_owned)
}

fn write_response(
    stream: &mut TcpStream,
    request_line: &str,
    version: &str,
    providers: &ProviderPlan,
) {
    let path = request_line.split_whitespace().nth(1).unwrap_or_default();
    let (status, body) = match path {
        "/global/health" => (
            "200 OK",
            format!(r#"{{"healthy":true,"version":"{version}"}}"#),
        ),
        "/session" => ("200 OK", "[]".to_string()),
        "/config" => ("200 OK", "{}".to_string()),
        "/provider" => ("200 OK", providers.body()),
        _ => ("404 Not Found", "{}".to_string()),
    };
    let payload = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(payload.as_bytes());
    let _ = stream.flush();
}

struct FakeService {
    port: u16,
    attach_url: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl FakeService {
    fn start(port: u16, version: &str, providers: ProviderPlan) -> Self {
        Self::try_start(port, version, providers).expect("ephemeral synthetic service must bind")
    }

    /// Binding the exact port selected by the lifecycle can lose an OS
    /// allocation race against parallel tests; a short bounded bind backoff
    /// absorbs that window, and a persistent conflict reports None so the
    /// scenario can start over on fresh ports.
    fn try_start(port: u16, version: &str, providers: ProviderPlan) -> Option<Self> {
        let mut listener = None;
        for _ in 0..10 {
            match TcpListener::bind(("127.0.0.1", port)) {
                Ok(bound) => {
                    listener = Some(bound);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("synthetic service bind failed: {error}"),
            }
        }
        let listener = listener?;
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let join = {
            let requests = Arc::clone(&requests);
            let stop = Arc::clone(&stop);
            let version = version.to_string();
            let providers = Arc::new(providers);
            // The synthetic service accepts blocking one connection at a time.
            // A polling accept loop would make every request wait for a timer
            // wake-up, and host timer coalescing can stretch a short sleep far
            // beyond the probe deadline the scenarios under test rely on.
            thread::spawn(move || {
                loop {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            if stop.load(Ordering::SeqCst) {
                                break;
                            }
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                            if let Some(line) = read_request_head(&mut stream) {
                                requests.lock().unwrap().push(line.clone());
                                write_response(&mut stream, &line, &version, &providers);
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
        };
        Some(Self {
            port,
            attach_url: format!("http://127.0.0.1:{port}"),
            requests,
            stop,
            join: Some(join),
        })
    }

    fn request_lines(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for FakeService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Release the blocking accept so the thread observes the stop flag and
        // drops the listener before the join below.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// A real child process standing in for an owned service generation. A reaper
/// thread waits on it immediately so an externally terminated child never
/// lingers as a zombie (which `kill -0` would still report as alive).
struct ChildGuard {
    pid: u32,
    child: Arc<Mutex<Option<Child>>>,
    stop: Arc<AtomicBool>,
    reaper: Option<thread::JoinHandle<()>>,
}

impl ChildGuard {
    fn spawn() -> Self {
        #[cfg(unix)]
        let child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        #[cfg(windows)]
        let child = Command::new("cmd")
            .args(["/C", "pause"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let child = Arc::new(Mutex::new(Some(child)));
        let stop = Arc::new(AtomicBool::new(false));
        let reaper = {
            let child = Arc::clone(&child);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                loop {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    {
                        let mut guard = child.lock().unwrap();
                        match guard.as_mut().map(|running| running.try_wait()) {
                            Some(Ok(Some(_))) | None => {
                                guard.take();
                                break;
                            }
                            Some(Ok(None)) => {}
                            Some(Err(_)) => break,
                        }
                    }
                    thread::sleep(Duration::from_millis(25));
                }
            })
        };
        Self {
            pid,
            child,
            stop,
            reaper: Some(reaper),
        }
    }

    fn id(&self) -> u32 {
        self.pid
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        {
            let mut guard = self.child.lock().unwrap();
            if let Some(mut running) = guard.take() {
                let _ = running.kill();
                let _ = running.wait();
            }
        }
        self.stop.store(true, Ordering::SeqCst);
        if let Some(reaper) = self.reaper.take() {
            let _ = reaper.join();
        }
    }
}

struct FakeRunner {
    service_version: String,
    provider_failures_before_ready: usize,
    bind_conflicts: AtomicUsize,
    spawns: Mutex<Vec<(String, u16)>>,
    services: Mutex<Vec<FakeService>>,
    children: Mutex<Vec<ChildGuard>>,
}

impl FakeRunner {
    fn new(service_version: &str, provider_failures_before_ready: usize) -> Self {
        Self {
            service_version: service_version.to_string(),
            provider_failures_before_ready,
            bind_conflicts: AtomicUsize::new(0),
            spawns: Mutex::new(Vec::new()),
            services: Mutex::new(Vec::new()),
            children: Mutex::new(Vec::new()),
        }
    }

    fn spawned_ports(&self) -> Vec<u16> {
        self.spawns
            .lock()
            .unwrap()
            .iter()
            .map(|(_, port)| *port)
            .collect()
    }

    fn child_pids(&self) -> Vec<u32> {
        self.children
            .lock()
            .unwrap()
            .iter()
            .map(ChildGuard::id)
            .collect()
    }
}

impl ServeRunner for FakeRunner {
    fn spawn(
        &self,
        executable: &str,
        _host: &str,
        port: u16,
        _configure: fn(&mut Command, &str, u16),
    ) -> Result<u32, SpawnFailure> {
        let plan = if self.provider_failures_before_ready == 0 {
            ProviderPlan::Fixed(READY_PROVIDERS.to_string())
        } else {
            ProviderPlan::FlipAfter {
                failures: AtomicUsize::new(self.provider_failures_before_ready),
                ready: READY_PROVIDERS.to_string(),
            }
        };
        let Some(service) = FakeService::try_start(port, &self.service_version, plan) else {
            self.bind_conflicts.fetch_add(1, Ordering::SeqCst);
            return Err(SpawnFailure::Start);
        };
        self.services.lock().unwrap().push(service);
        self.spawns
            .lock()
            .unwrap()
            .push((executable.to_string(), port));
        let child = ChildGuard::spawn();
        let pid = child.id();
        self.children.lock().unwrap().push(child);
        Ok(pid)
    }
}

fn temp_paths(label: &str) -> (std::path::PathBuf, ServicePaths) {
    let root = std::env::temp_dir().join(format!(
        "lico-local-service-serve-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let paths = ServicePaths::from_root(root.clone(), "serve.pid").unwrap();
    (root, paths)
}

fn current_executable() -> String {
    std::env::current_exe()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn seed_owned_generation(paths: &ServicePaths, pid: u32, attach_url: &str, port: u16) {
    state::write_json(
        &paths.state_path,
        &json!({
            "schemaVersion": "synthetic-v1",
            "status": "running",
            "running": true,
            "owned": true,
            "pid": pid,
            "host": "127.0.0.1",
            "port": port,
            "preferredPort": port,
            "attachUrl": attach_url,
            "portConflict": false,
            "launchIdentity": {
                "file": "synthetic-old-file-identity",
                "version": "1.0.0"
            },
            "draining": [],
            "updatedAtUnix": 0
        }),
    )
    .unwrap();
}

fn persisted_state(paths: &ServicePaths) -> Value {
    let text = std::fs::read_to_string(&paths.state_path).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// Returns false only when the synthetic runner lost an OS port-allocation
/// race; the test then retries the whole scenario once on fresh fixtures.
fn draining_scenario_attempt() -> bool {
    let (root, paths) = temp_paths("draining");
    let old_service =
        FakeService::start(0, "1.0.0", ProviderPlan::Fixed(READY_PROVIDERS.to_string()));
    let old_child = ChildGuard::spawn();
    let old_pid = old_child.id();
    seed_owned_generation(&paths, old_pid, &old_service.attach_url, old_service.port);
    // An active attachment reference pins the old generation's endpoint.
    let lease = turn_control::pin_endpoint(&old_service.attach_url).unwrap();
    let runner = FakeRunner::new("2.0.0", 0);
    let spec = synthetic_spec(old_service.port);
    let params = json!({
        "executable": current_executable(),
        "port": old_service.port,
        "healthTimeoutMs": 5_000
    });

    // Identity mismatch with an active endpoint: the old generation moves to
    // draining untouched, and a ready replacement is promoted elsewhere.
    let first = serve::start_with_paths_for_test(spec, &params, &paths, true, &runner).unwrap();
    if runner.bind_conflicts.load(Ordering::SeqCst) > 0 {
        return false;
    }
    assert_eq!(first["ok"], json!(true));
    assert_eq!(first["healthy"], json!(true));
    let replacement_url = first["attachUrl"].as_str().unwrap().to_string();
    assert_ne!(replacement_url, old_service.attach_url);
    assert_eq!(runner.spawned_ports().len(), 1);
    let replacement_pid = runner.child_pids()[0];
    assert!(process::alive(Some(old_pid)));
    assert!(process::alive(Some(replacement_pid)));
    let state = persisted_state(&paths);
    assert_eq!(state["pid"], json!(replacement_pid));
    assert_eq!(state["launchIdentity"]["version"], json!("2.0.0"));
    assert_eq!(state["draining"].as_array().unwrap().len(), 1);
    assert_eq!(state["draining"][0]["pid"], json!(old_pid));
    assert_eq!(
        state["draining"][0]["attachUrl"],
        json!(old_service.attach_url)
    );

    // Future dispatches reuse only the ready current replacement; the active
    // old generation still receives no terminate call.
    let second = serve::start_with_paths_for_test(spec, &params, &paths, true, &runner).unwrap();
    assert_eq!(second["reused"], json!(true));
    assert_eq!(second["attachUrl"], json!(replacement_url));
    assert_eq!(runner.spawned_ports().len(), 1);
    assert!(process::alive(Some(old_pid)));

    // Once the active reference is gone, a later lifecycle operation cleans
    // the drained owned process while the current generation keeps serving.
    drop(lease);
    let third = serve::start_with_paths_for_test(spec, &params, &paths, true, &runner).unwrap();
    assert_eq!(third["reused"], json!(true));
    assert!(!process::alive(Some(old_pid)));
    assert!(process::alive(Some(replacement_pid)));
    let state = persisted_state(&paths);
    assert_eq!(state["draining"].as_array().unwrap().len(), 0);
    assert_eq!(state["pid"], json!(replacement_pid));

    drop(runner);
    drop(old_child);
    drop(old_service);
    let _ = std::fs::remove_dir_all(root);
    true
}

#[cfg(unix)]
#[test]
fn active_generation_drains_while_replacement_serves_future_turns() {
    for attempt in 0..2 {
        if draining_scenario_attempt() {
            return;
        }
        assert_eq!(attempt, 0, "repeated synthetic port-allocation conflicts");
    }
}

#[cfg(unix)]
#[test]
fn readiness_waits_for_session_and_model_binding_and_posts_no_message() {
    let service = FakeService::start(
        0,
        "9.9.9",
        ProviderPlan::FlipAfter {
            failures: AtomicUsize::new(3),
            ready: READY_PROVIDERS.to_string(),
        },
    );
    let spec = synthetic_spec(service.port);
    // While the provider catalog is not usable, readiness fails and no
    // attachment can be handed out by the ensure path.
    assert!(
        serve::probe_ready_for_test(spec, &service.attach_url, Duration::from_millis(500)).is_err()
    );
    // Once health, the session collection, and the current model binding are
    // all usable, one observation succeeds.
    let mut readiness = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while readiness.is_none() && Instant::now() < deadline {
        readiness =
            serve::probe_ready_for_test(spec, &service.attach_url, Duration::from_millis(500)).ok();
        if readiness.is_none() {
            thread::sleep(Duration::from_millis(25));
        }
    }
    let readiness = readiness.expect("readiness must hold once every fact is usable");
    assert_eq!(readiness.version, "9.9.9");
    assert_eq!(
        readiness.catalog.current.selector(),
        "synthetic-provider/current-model"
    );
    // The barrier is read-only: only health/session/config/provider GETs ever
    // left the endpoint; no message POST appears before or after readiness.
    let requests = service.request_lines();
    assert!(!requests.is_empty());
    assert!(
        requests.iter().all(|line| line.starts_with("GET ")),
        "requests: {requests:?}"
    );
    let provider_hits = requests
        .iter()
        .filter(|line| line.starts_with("GET /provider"))
        .count();
    assert!(provider_hits >= 4, "requests: {requests:?}");
}

/// Returns false only when the synthetic runner lost an OS port-allocation
/// race; the test then retries the whole scenario once on fresh fixtures.
fn quiescent_scenario_attempt() -> bool {
    let (root, paths) = temp_paths("quiescent");
    let old_service =
        FakeService::start(0, "1.0.0", ProviderPlan::Fixed(READY_PROVIDERS.to_string()));
    let old_child = ChildGuard::spawn();
    let old_pid = old_child.id();
    seed_owned_generation(&paths, old_pid, &old_service.attach_url, old_service.port);
    let runner = FakeRunner::new("2.0.0", 0);
    let spec = synthetic_spec(old_service.port);
    let params = json!({
        "executable": current_executable(),
        "port": old_service.port,
        "healthTimeoutMs": 5_000
    });
    let result = serve::start_with_paths_for_test(spec, &params, &paths, true, &runner).unwrap();
    if runner.bind_conflicts.load(Ordering::SeqCst) > 0 {
        return false;
    }
    assert_eq!(result["ok"], json!(true));
    assert_ne!(
        result["attachUrl"].as_str().unwrap(),
        old_service.attach_url
    );
    assert!(!process::alive(Some(old_pid)));
    let replacement_pid = runner.child_pids()[0];
    assert!(process::alive(Some(replacement_pid)));
    let state = persisted_state(&paths);
    assert_eq!(state["pid"], json!(replacement_pid));
    assert_eq!(state["draining"].as_array().unwrap().len(), 0);
    drop(runner);
    drop(old_child);
    drop(old_service);
    let _ = std::fs::remove_dir_all(root);
    true
}

#[cfg(unix)]
#[test]
fn quiescent_mismatch_stops_the_old_owned_generation_and_replaces_it() {
    for attempt in 0..2 {
        if quiescent_scenario_attempt() {
            return;
        }
        assert_eq!(attempt, 0, "repeated synthetic port-allocation conflicts");
    }
}
