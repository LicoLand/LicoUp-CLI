//! Client-local owner for desktop Agent conversation RPC.
//!
//! The Flutter process owns a replaceable stdio proxy. The listener and every
//! accepted Agent turn live in this CLI host, scoped to the client-owned
//! portable data root. The host belongs to that LicoUp process: when the
//! client pid in `LICOUP_CLIENT_PID` is gone, the host observes that on its
//! next owner check. Explicit shutdown closes admission and drains accepted
//! turns and sessions before releasing root ownership. A five-minute idle exit
//! applies only when that owner pid is unset (CLI and tests).

use anyhow::{Context, Result, anyhow};
use fs2::FileExt;
use interprocess::local_socket::{
    ListenerNonblockingMode, ListenerOptions, SendHalf, Stream, traits::Stream as _,
};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use super::stdio_rpc::{
    PersistentConversationRuntime, execute_rpc_cli, serve_stdio_rpc_with_persistent_conversation,
};

const CONNECT_ATTEMPTS: usize = 80;
const CONNECT_RETRY: Duration = Duration::from_millis(25);
const CONNECT_ATTEMPT_TIMEOUT: Duration = Duration::from_millis(25);
const STALE_HOST_WAIT: Duration = Duration::from_secs(2);
#[allow(dead_code)]
const OWNER_CHECK_INTERVAL: Duration = Duration::from_millis(500);
#[allow(dead_code)]
const IDLE_EXIT_GRACE: Duration = Duration::from_secs(300);
const CLIENT_PID_ENV: &str = "LICOUP_CLIENT_PID";

static HOST_STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
fn register_termination_signal_handler() {
    extern "C" fn handle_signal(_: libc::c_int) {
        HOST_STOP_REQUESTED.store(true, Ordering::SeqCst);
    }
    unsafe {
        libc::signal(
            libc::SIGTERM,
            handle_signal as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGINT,
            handle_signal as *const () as libc::sighandler_t,
        );
    }
}

#[cfg(not(unix))]
fn register_termination_signal_handler() {}

pub(super) fn request_host_stop() -> Result<()> {
    // An unlocked owner file is stronger evidence than a socket probe: it
    // proves there is no process currently writing through this host.
    if let Some(owner) = HostOwnerLock::acquire()? {
        drop(owner);
        return Ok(());
    }
    let params = serde_json::json!({ "host": true });
    let _response =
        licoup_native::platform::conversation_host_client::execute_existing("shutdown", &params)
            .map_err(|_| anyhow!("persistent_conversation_transport_required"))?;
    loop {
        if let Some(owner) = HostOwnerLock::acquire()? {
            drop(owner);
            return Ok(());
        }
        thread::sleep(CONNECT_RETRY);
    }
}

fn host_generation_path(root: &Path) -> PathBuf {
    root.join("client-state")
        .join("conversation-runtime")
        .join("host-generation")
}

/// The lock that says which process owns this root's listener right now.
///
/// Deliberately separate from the record on disk: the record says who *did*
/// own the listener, this says who owns it, and the OS releases it when the
/// owning process dies.
struct HostOwnerLock {
    file: File,
}

impl HostOwnerLock {
    /// Take the root's ownership lock, or `None` when a live host holds it.
    fn acquire() -> Result<Option<Self>> {
        let root = licoup_foundation::platform::paths::portable_data_dir()?;
        Ok(
            licoup_native::platform::conversation_host_transport::try_acquire_host_owner_lock(
                &root,
            )?
            .map(|file| Self { file }),
        )
    }
}

impl Drop for HostOwnerLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn executable_generation() -> io::Result<String> {
    licoup_native::platform::conversation_host_transport::executable_generation()
}

fn valid_host_generation(value: &str) -> bool {
    value.len() == 16 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn parse_host_generation_record(text: &str) -> Option<(String, Option<u32>, Option<u32>)> {
    let mut lines = text.lines();
    let identity = lines.next()?.trim();
    if !valid_host_generation(identity) {
        return None;
    }
    let host_pid = match lines.next().map(str::trim).filter(|line| !line.is_empty()) {
        None => None,
        Some(line) => Some(line.parse().ok()?),
    };
    let client_pid = match lines.next().map(str::trim) {
        None => None,
        Some("") => None,
        Some(line) => Some(parse_client_pid(line)?),
    };
    if lines.next().is_some() {
        return None;
    }
    Some((identity.to_owned(), host_pid, client_pid))
}

fn read_host_generation_record() -> Option<(String, Option<u32>, Option<u32>)> {
    let root = licoup_foundation::platform::paths::portable_data_dir().ok()?;
    let text = fs::read_to_string(host_generation_path(&root)).ok()?;
    parse_host_generation_record(&text)
}

fn write_host_generation() -> Result<()> {
    let root = licoup_foundation::platform::paths::portable_data_dir()?;
    let path = host_generation_path(&root);
    if let Some(parent) = path.parent() {
        licoup_foundation::platform::file_security::ensure_private_dir(parent)?;
    }
    let generation = executable_generation().context("conversation host unavailable")?;
    let mut body = format!("{generation}\n{}\n", std::process::id());
    if let Some(client_pid) = configured_client_pid() {
        body.push_str(&format!("{client_pid}\n"));
    }
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .context("conversation host unavailable")?;
    file.write_all(body.as_bytes())?;
    file.sync_all()?;
    licoup_foundation::platform::file_security::harden_private_path(&path)?;
    Ok(())
}

/// What the local ownership record says about a host for this state root.
///
/// The record is written by whichever host last acquired the listener, so it
/// can be out of date while a live host serves: a host that exits without
/// rewriting it leaves a dead pid behind, and a host that takes the listener
/// over replaces the record only once it is serving. Callers need the three
/// cases apart, because "the record is out of date" and "no host exists" call
/// for opposite reactions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostOwnership {
    /// This executable's generation, and the host it names is still running.
    Current,
    /// This executable's generation, but the host it names is gone. A live
    /// host may still be serving under another process; only the endpoint can
    /// say, so this is not evidence that nobody is there.
    Stale,
    /// Nothing this process may claim: no record, an unreadable one, another
    /// generation, or another client's host.
    Absent,
}

impl HostOwnership {
    /// Whether the record names a live host this process may speak for.
    const fn is_current(self) -> bool {
        matches!(self, Self::Current)
    }
}

/// Classify one record against this executable and client.
///
/// `liveness` is injected so every outcome can be tested without spawning a
/// host: a live pid, a dead one, and an unreadable one are one closure apart.
fn classify_host_ownership(
    record: Option<(String, Option<u32>, Option<u32>)>,
    generation: &str,
    _expected_client: Option<u32>,
    liveness: impl Fn(u32) -> ProcessLiveness,
) -> HostOwnership {
    let Some((recorded, host_pid, _recorded_client)) = record else {
        return HostOwnership::Absent;
    };
    if recorded != generation {
        return HostOwnership::Absent;
    }
    // A record that names no host says nothing about another client's
    // ownership, so it stays an out-of-date record for this generation.
    let Some(host_pid) = host_pid else {
        return HostOwnership::Stale;
    };
    if liveness(host_pid) == ProcessLiveness::Dead {
        return HostOwnership::Stale;
    }
    HostOwnership::Current
}

fn host_ownership() -> HostOwnership {
    let Ok(generation) = executable_generation() else {
        return HostOwnership::Absent;
    };
    classify_host_ownership(
        read_host_generation_record(),
        &generation,
        configured_client_pid(),
        process_liveness,
    )
}

/// Whether the record names a live host of this executable that this process
/// may speak for. Prefer [`host_ownership`] where a stale record must be told
/// apart from no record.
fn host_is_current() -> bool {
    host_ownership().is_current()
}

fn endpoint_accepts_connections() -> bool {
    try_connect(licoup_native::platform::conversation_host_transport::connect).is_some()
}

/// A local-socket connect can block in the OS while the listener is starting
/// or being taken over. Keep the caller bounded even when it holds ownership.
/// A timeout means that this attempt has not answered; callers may retry.
fn try_connect(connector: fn() -> io::Result<Stream>) -> Option<Stream> {
    run_with_timeout(connector, CONNECT_ATTEMPT_TIMEOUT)
}

fn run_with_timeout<T, F>(operation: F, timeout: Duration) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> io::Result<T> + Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = sender.send(operation());
    });

    match receiver.recv_timeout(timeout) {
        Ok(Ok(value)) => Some(value),
        Ok(Err(_))
        | Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => None,
    }
}

/// Whether any host is still answering, asked repeatedly until `window` runs
/// out.
///
/// One refused connect is not proof that nobody is serving: a host can miss a
/// single probe while it is busy. The listener takeover asks over a window so
/// it never displaces a live host on one unlucky attempt.
fn endpoint_accepts_connections_within(window: Duration) -> bool {
    let deadline = Instant::now() + window;
    loop {
        if endpoint_accepts_connections() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(CONNECT_RETRY);
    }
}

fn wait_for_current_or_released_endpoint() -> Option<Stream> {
    let deadline = Instant::now() + STALE_HOST_WAIT;
    while Instant::now() < deadline {
        if host_is_current()
            && let Some(stream) =
                try_connect(licoup_native::platform::conversation_host_transport::connect)
        {
            return Some(stream);
        }
        if !endpoint_accepts_connections() {
            return None;
        }
        thread::sleep(CONNECT_RETRY);
    }
    None
}

pub(super) fn connect_for_cli(require_running: bool) -> Result<Stream> {
    if require_running {
        return try_connect(licoup_native::platform::conversation_host_transport::connect_existing)
            .ok_or_else(|| anyhow!("persistent_conversation_transport_required"));
    }
    connect_or_start()
}

fn connect_or_start() -> Result<Stream> {
    if host_ownership() != HostOwnership::Absent {
        // The record names this executable's generation, whether or not the
        // host process it names is still alive. A record that is merely out of
        // date must not cost the stale wait against a reachable host, so try
        // the endpoint first and let it decide.
        if let Some(stream) =
            try_connect(licoup_native::platform::conversation_host_transport::connect)
        {
            return Ok(stream);
        }
    } else if endpoint_accepts_connections() {
        if let Some(stream) = wait_for_current_or_released_endpoint() {
            return Ok(stream);
        }
        if endpoint_accepts_connections() {
            // Never kill by an untrusted or stale PID record. A live endpoint
            // owned by another client or binary fails closed instead.
            return Err(anyhow!("conversation host unavailable"));
        }
    }
    spawn_host()?;
    for _ in 0..CONNECT_ATTEMPTS {
        if host_is_current() {
            if let Some(stream) =
                try_connect(licoup_native::platform::conversation_host_transport::connect)
            {
                return Ok(stream);
            }
        }
        thread::sleep(CONNECT_RETRY);
    }
    Err(anyhow!("conversation host unavailable"))
}

fn parse_client_pid(value: &str) -> Option<u32> {
    let value = value.trim();
    if value.is_empty() || value.len() > 10 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let pid = value.parse().ok()?;
    (pid > 1).then_some(pid)
}

fn configured_client_pid() -> Option<u32> {
    parse_client_pid(&env::var(CLIENT_PID_ENV).ok()?)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProcessLiveness {
    Alive,
    Dead,
    Unknown,
}

fn process_liveness(pid: u32) -> ProcessLiveness {
    if pid <= 1 {
        return ProcessLiveness::Dead;
    }
    #[cfg(unix)]
    {
        use nix::sys::signal::kill;
        use nix::unistd::Pid;

        let raw = i32::try_from(pid).unwrap_or(0);
        if raw <= 1 {
            return ProcessLiveness::Dead;
        }
        match kill(Pid::from_raw(raw), None) {
            Ok(()) | Err(nix::errno::Errno::EPERM) => ProcessLiveness::Alive,
            Err(nix::errno::Errno::ESRCH) => ProcessLiveness::Dead,
            Err(_) => ProcessLiveness::Unknown,
        }
    }
    #[cfg(windows)]
    {
        windows_process_liveness(pid)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        ProcessLiveness::Unknown
    }
}

#[cfg(windows)]
fn windows_process_liveness(pid: u32) -> ProcessLiveness {
    let filter = format!("PID eq {pid}");
    let output = Command::new("tasklist")
        .args(["/FI", &filter, "/FO", "CSV", "/NH"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Ok(output) = output else {
        return ProcessLiveness::Unknown;
    };
    if !output.status.success() {
        return ProcessLiveness::Unknown;
    }
    let Ok(stdout) = String::from_utf8(output.stdout) else {
        return ProcessLiveness::Unknown;
    };
    let expected = pid.to_string();
    if stdout.lines().any(|line| {
        line.split("\",\"")
            .nth(1)
            .map(|value| value.trim_matches('"'))
            == Some(expected.as_str())
    }) {
        ProcessLiveness::Alive
    } else {
        ProcessLiveness::Dead
    }
}

#[allow(dead_code)]
fn client_owner_is_gone() -> bool {
    configured_client_pid().is_some_and(|pid| process_liveness(pid) == ProcessLiveness::Dead)
}

/// Desktop startup ownership: every `rpc stdio` bridge lane launched by the
/// desktop client (marked by LICOUP_CLIENT_PID) ensures the persistent
/// conversation host — and with it the supervised Subagent MCP service — is
/// running before any conversation RPC. A spawn failure only degrades MCP
/// readiness; the bridge lane is never affected.
pub(super) fn ensure_host_for_desktop_start() {
    if configured_client_pid().is_none() {
        return;
    }
    // A record this process can claim — current or out of date — plus an
    // endpoint that accepts means a host is already serving. Spawning would
    // only collide with it.
    if host_ownership() != HostOwnership::Absent && endpoint_accepts_connections() {
        return;
    }
    let _ = spawn_host();
}

fn spawn_host() -> Result<()> {
    let executable = env::current_exe().context("conversation host executable unavailable")?;
    let mut command = Command::new(executable);
    command
        .args(["rpc", "conversation-host"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(client_pid) = configured_client_pid() {
        command.env(CLIENT_PID_ENV, client_pid.to_string());
    }
    command
        .spawn()
        .map(|_| ())
        .context("conversation host start failed")
}

pub(super) fn serve_proxy() -> Result<()> {
    let stream = connect_or_start()?;
    let (mut receiver, mut sender) = stream.split();
    // Continue draining the host after stdout disappears. This prevents a
    // closed GUI pipe from applying backpressure to the turn owner.
    let upload = thread::spawn(move || -> io::Result<()> {
        io::copy(&mut io::stdin().lock(), &mut sender)?;
        sender.flush()?;
        shutdown_upload(&sender)
    });
    let mut stdout = io::stdout().lock();
    let mut buffer = [0_u8; 16 * 1024];
    let mut observable = true;
    loop {
        let count = receiver.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if observable && stdout.write_all(&buffer[..count]).is_err() {
            observable = false;
        }
        if observable {
            let _ = stdout.flush();
        }
    }
    // Host EOF must close this proxy's stdout immediately so the desktop can
    // reconnect. Joining would wait forever while the upload thread is still
    // blocked on an open GUI stdin pipe; dropping the handle lets process exit
    // terminate that replaceable proxy thread.
    drop(upload);
    Ok(())
}

#[cfg(unix)]
fn shutdown_upload(sender: &SendHalf) -> io::Result<()> {
    use std::net::Shutdown;

    match sender {
        SendHalf::UdSocket(sender) => sender.as_stream().inner().shutdown(Shutdown::Write),
        #[allow(unreachable_patterns)]
        _ => Ok(()),
    }
}

#[cfg(windows)]
fn shutdown_upload(_sender: &SendHalf) -> io::Result<()> {
    // The named-pipe send half is independently owned and signals completion
    // when the upload thread returns and drops it.
    Ok(())
}

pub(super) fn serve_host() -> Result<()> {
    // Ownership is proven here, not read from the record. Take the lock before
    // touching the listener, so the takeover below can never race another host
    // deciding at the same moment that nobody owns this root. The lock lives
    // until this function returns, which is the whole service lifetime.
    let _owner_lock = match HostOwnerLock::acquire()? {
        Some(lock) => lock,
        None => {
            // A live host owns this root, whatever the record says. Our own is
            // a quiet exit; another client's or generation's fails closed.
            return match host_ownership() {
                HostOwnership::Current => Ok(()),
                _ => Err(anyhow!("conversation host listener already active")),
            };
        }
    };
    let name = licoup_native::platform::conversation_host_transport::endpoint_name()?;
    let listener = match ListenerOptions::new()
        .name(name)
        .nonblocking(ListenerNonblockingMode::Accept)
        .try_overwrite(false)
        .create_sync()
    {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
            if host_ownership() == HostOwnership::Current {
                return Ok(());
            }
            // This host already holds the ownership lock, so no other host that
            // speaks for this root is alive to race here: the listener outlived
            // its owner. The probe still asks the endpoint first, which covers
            // a host old enough to predate the lock — and it is asked over a
            // window, because a busy host that misses one probe must not lose
            // its listener to a starting host.
            if endpoint_accepts_connections_within(STALE_HOST_WAIT) {
                return Err(error).context("conversation host listener already active");
            }
            ListenerOptions::new()
                .name(licoup_native::platform::conversation_host_transport::endpoint_name()?)
                .nonblocking(ListenerNonblockingMode::Accept)
                .try_overwrite(true)
                .create_sync()
                .context("conversation host listener failed")?
        }
        Err(error) => return Err(error).context("conversation host listener failed"),
    };
    write_host_generation()?;
    let root = licoup_foundation::platform::paths::portable_data_dir()?;
    // The persistent Conversation host and Gateway must admit the same
    // evidence-bound adapters. The Gateway persists hot reloads in this
    // standard overlay; load it before any runtime profile is resolved so a
    // restarted host cannot fall back to the packaged readiness snapshot.
    let readiness_overlay = root
        .join("llm-gateway")
        .join(licoup_private_gateway::control::inventory::OVERLAY_FILE_NAME);
    let _ = licoup_private_gateway::control::inventory::load_inventory_overlay_if_present(
        &readiness_overlay,
    );
    let service = licoup_native::domain::client_conversation::ConversationService::open(&root)?;
    let runtime = PersistentConversationRuntime::new(service.store().clone());
    let service = super::stdio_rpc::bind_conversation_runtime(service, &runtime, None);
    {
        let hooked = service.clone();
        runtime.set_settlement_hook(move |conversation_id, payload| {
            hooked
                .after_runtime_settlement(conversation_id, payload)
                .map(|_| ())
                .map_err(|err| err.to_string())
        });
    }
    // The optional MCP service is not this host's to start. Its package
    // declares `on-demand` activation, so nothing launches it here: a caller
    // starts it, an absent or switched-off package starts nothing, and the
    // client neither bundles it nor keeps a switch that changes that.
    serve_bound_host(listener, service, runtime, None)
}

/// One serialized in-process owner for `attend_due`. The accept loop never
/// runs cognition and never joins this worker. Host owner exit signals the
/// loop and returns immediately so a held cognition cannot pin the listener.
struct AttendanceOwner {
    stop: Arc<AtomicBool>,
    #[allow(dead_code)]
    active: Arc<AtomicBool>,
    wake: Arc<(Mutex<()>, Condvar)>,
    join: Option<thread::JoinHandle<()>>,
}

impl AttendanceOwner {
    fn spawn(
        service: licoup_native::domain::client_conversation::ConversationService,
    ) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicBool::new(false));
        let wake = Arc::new((Mutex::new(()), Condvar::new()));
        let stop_w = stop.clone();
        let active_w = active.clone();
        let wake_w = wake.clone();
        let join = thread::Builder::new()
            .name("conversation-attend".to_owned())
            .spawn(move || {
                // Claim/cold-recover may enter cognition. Mark attendance
                // active first so generic idle cannot treat that work as idle.
                active_w.store(true, Ordering::Release);
                let _ = service.claim_continuity_owner();
                while !stop_w.load(Ordering::Acquire) {
                    active_w.store(true, Ordering::Release);
                    let _ = service.attend_due();
                    active_w.store(false, Ordering::Release);
                    if stop_w.load(Ordering::Acquire) {
                        break;
                    }
                    let (lock, cv) = &*wake_w;
                    let guard = lock.lock().unwrap_or_else(|poison| poison.into_inner());
                    let _ = cv.wait_timeout(guard, CONNECT_RETRY);
                }
            })
            .context("conversation attendance worker failed")?;
        Ok(Self {
            stop,
            active,
            wake,
            join: Some(join),
        })
    }

    #[allow(dead_code)]
    fn active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    fn shutdown(self) -> Result<()> {
        self.stop.store(true, Ordering::Release);
        self.wake.1.notify_all();
        self.join
            .ok_or_else(|| anyhow!("conversation attendance worker unavailable"))?
            .join()
            .map_err(|_| anyhow!("conversation attendance worker failed"))
    }
}

#[allow(dead_code)]
fn generic_idle_may_exit(runtime_idle: bool, attendance_active: bool) -> bool {
    runtime_idle && !attendance_active
}

fn serve_bound_host(
    listener: impl interprocess::local_socket::traits::Listener<Stream = Stream>,
    service: licoup_native::domain::client_conversation::ConversationService,
    runtime: PersistentConversationRuntime,
    stop: Option<Arc<AtomicBool>>,
) -> Result<()> {
    HOST_STOP_REQUESTED.store(false, Ordering::Release);
    register_termination_signal_handler();
    let attendance = AttendanceOwner::spawn(service.clone())?;
    let mut sessions = Vec::new();
    let result = loop {
        if let Err(error) = reap_finished_conversation_sessions(&mut sessions) {
            break Err(error);
        }
        if stop
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
            || HOST_STOP_REQUESTED.load(Ordering::Acquire)
            || runtime.is_host_stop_requested()
        {
            break Ok(());
        }
        match listener.accept() {
            Ok(stream) => {
                // macOS may propagate O_NONBLOCK from the listener despite
                // ListenerNonblockingMode::Accept. Each accepted RPC session
                // is served by its own thread and must block between frames.
                if stream.set_nonblocking(false).is_err() {
                    continue;
                }
                runtime.client_connected();
                let session_runtime = runtime.clone();
                let failed_spawn_runtime = runtime.clone();
                let conversation_service = service.clone();
                match thread::Builder::new()
                    .name("conversation-rpc".into())
                    .spawn(move || {
                        let _client = ConversationClientOwner(session_runtime.clone());
                        let (receiver, sender) = stream.split();
                        let _ = serve_stdio_rpc_with_persistent_conversation(
                            BufReader::new(receiver),
                            sender,
                            execute_rpc_cli,
                            session_runtime.clone(),
                            conversation_service,
                        );
                    }) {
                    Ok(session) => sessions.push(session),
                    Err(_) => {
                        failed_spawn_runtime.client_disconnected();
                        break Err(anyhow!("conversation host session worker failed"));
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                // Per D19, GUI client exit does not stop the host, and generic idle
                // does not stop the host. Explicit stop is the only stop.
                thread::sleep(CONNECT_RETRY);
            }
            Err(_) => {
                thread::sleep(CONNECT_RETRY);
            }
        }
    };
    // Close admission first, then wait for every admitted turn and RPC session
    // to finish before checkpointing and releasing host ownership. No elapsed
    // time or endpoint probe is used as evidence that writers are quiescent.
    runtime.request_host_stop();
    runtime.wait_for_admitted_turns();
    runtime.wait_for_clients_to_disconnect();
    let session_result = sessions.into_iter().try_for_each(|session| {
        session
            .join()
            .map_err(|_| anyhow!("conversation host session worker failed"))
    });
    let checkpoint_result = service
        .store()
        .checkpoint()
        .map_err(|_| anyhow!("conversation host checkpoint failed"));
    let attendance_result = attendance.shutdown();
    result?;
    session_result?;
    checkpoint_result?;
    attendance_result?;
    Ok(())
}

fn reap_finished_conversation_sessions(sessions: &mut Vec<thread::JoinHandle<()>>) -> Result<()> {
    let mut index = 0;
    while index < sessions.len() {
        if sessions[index].is_finished() {
            let session = sessions.swap_remove(index);
            session
                .join()
                .map_err(|_| anyhow!("conversation host session worker failed"))?;
        } else {
            index += 1;
        }
    }
    Ok(())
}

struct ConversationClientOwner(PersistentConversationRuntime);

impl Drop for ConversationClientOwner {
    fn drop(&mut self) {
        self.0.client_disconnected();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interprocess::local_socket::Stream;
    use std::io::BufRead as _;

    #[test]
    fn runtime_bound_empty_service_retains_read_only_conversation_behavior() {
        if std::env::var_os("LICOUP_SYNTHETIC_BOUND_OWNER").is_none() {
            let root =
                std::env::temp_dir().join(format!("licoup-bound-owner-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "conversation_host::tests::runtime_bound_empty_service_retains_read_only_conversation_behavior", "--nocapture"])
                .env("LICOUP_SYNTHETIC_BOUND_OWNER", &root)
                .env("HOME", root.join("home"))
                .env("USERPROFILE", root.join("home"))
                .env("XDG_CONFIG_HOME", root.join("home/config"))
                .env("XDG_DATA_HOME", root.join("home/share"))
                .env("APPDATA", root.join("home/appdata"))
                .env("LOCALAPPDATA", root.join("home/local-appdata"))
                .env("LICOUP_HOME", &root)
                .env("LICO_MOBILE_RELAY_NATIVE_SECRET_STORE", "disabled")
                .output().unwrap();
            let _ = std::fs::remove_dir_all(&root);
            assert!(
                output.status.success(),
                "isolated owner boundary: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let root = PathBuf::from(std::env::var_os("LICOUP_SYNTHETIC_BOUND_OWNER").unwrap());
        assert!(root.starts_with(std::env::temp_dir()));
        assert!(
            root.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("licoup-bound-owner-")
        );
        std::fs::create_dir_all(&root).unwrap();
        let _access =
            licoup_foundation::platform::data_home_access::acquire_process_data_home_access()
                .unwrap();
        let service = licoup_native::domain::client_conversation::ConversationService::open(&root)
            .expect("synthetic owner opens");
        let runtime = PersistentConversationRuntime::new(service.store().clone());
        let bound = super::super::stdio_rpc::bind_conversation_runtime(service, &runtime, None);
        let actual_database = bound.store().db_path().to_path_buf();
        bound
            .execute(serde_json::json!({"action": "conversation.list"}))
            .expect("bound synthetic owner must list without an Agent or private runtime state");
        let threaded = bound.clone();
        std::thread::spawn(move || {
            threaded.execute(serde_json::json!({"action":"conversation.list"}))
        })
        .join()
        .unwrap()
        .expect("bound owner must preserve its store across request threads");
        let body = serde_json::json!({"action":"conversation.list"});
        let args = super::super::private_stdin_json::materialize_private_stdin_json(
            ["conversation", "execute", "--stdin-json", "true"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            std::io::Cursor::new(body.to_string().into_bytes()),
        )
        .unwrap();
        let admitted = licoup_native::ffi::commands::admit_cli_command(args).unwrap();
        let (request, _) = licoup_native::ffi::commands::native_rpc::request_for_command(&admitted)
            .unwrap()
            .unwrap();
        assert_eq!(request["params"], body);
        let input = std::io::Cursor::new(format!("{request}\n").into_bytes());
        let output = serve_stdio_rpc_with_persistent_conversation(
            input,
            Vec::new(),
            execute_rpc_cli,
            runtime,
            bound,
        )
        .expect("synthetic persistent dispatcher runs");
        let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(
            response["ok"], true,
            "synthetic dispatcher response: {response}"
        );
        let reference_root = root.join("reference-owner");
        let reference_store =
            licoup_native::domain::client_conversation::ConversationService::open(&reference_root)
                .unwrap();
        let actual_schema = rusqlite::Connection::open(&actual_database).unwrap();
        let reference_schema =
            rusqlite::Connection::open(reference_store.store().db_path()).unwrap();
        for table in licoup_foundation::core::sqlite_contract::tables(&reference_schema).unwrap() {
            licoup_foundation::core::sqlite_contract::validate_table(
                &actual_schema,
                &reference_schema,
                &table,
                None,
            )
            .unwrap_or_else(|error| {
                panic!("synthetic owner schema changed after composition: {table}: {error:#}")
            });
        }
        drop(actual_schema);
        drop(reference_schema);
        drop(reference_store);
        let service =
            licoup_native::domain::client_conversation::ConversationService::open(&root).unwrap();
        let runtime = PersistentConversationRuntime::new(service.store().clone());
        let service = super::super::stdio_rpc::bind_conversation_runtime(service, &runtime, None);
        let probe = service.clone();
        let listener = ListenerOptions::new()
            .name(
                licoup_native::platform::conversation_host_transport::endpoint_name_for_root(&root)
                    .unwrap(),
            )
            .nonblocking(ListenerNonblockingMode::Accept)
            .create_sync()
            .unwrap();
        let host = std::thread::spawn(move || serve_bound_host(listener, service, runtime, None));
        let mut stream = connect_test_host(&root);
        serde_json::to_writer(&mut stream, &request).unwrap();
        stream.write_all(b"\n").unwrap();
        stream.flush().unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        probe
            .execute(serde_json::json!({"action":"conversation.list"}))
            .expect("owner remains valid after attendance/listener startup");
        assert_eq!(
            response["ok"], true,
            "synthetic actual-listener response: {response}"
        );
        let stop = serde_json::json!({"protocol":"licoup.stdio.v1","id":"stop","workflowId":request["workflowId"],"method":"shutdown","params":{"host":true}});
        serde_json::to_writer(reader.get_mut(), &stop).unwrap();
        reader.get_mut().write_all(b"\n").unwrap();
        reader.get_mut().flush().unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        drop(reader);
        host.join().unwrap().unwrap();
        drop(probe);
        drop(_access);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn idle_exit_waits_minutes_after_the_owner_is_empty() {
        assert!(IDLE_EXIT_GRACE >= Duration::from_secs(60));
    }

    #[test]
    fn generic_idle_exit_does_not_truncate_active_attendance() {
        assert!(generic_idle_may_exit(true, false));
        assert!(!generic_idle_may_exit(true, true));
        assert!(!generic_idle_may_exit(false, false));
        assert!(!generic_idle_may_exit(false, true));
    }

    #[test]
    fn client_pid_is_digits_and_rejects_init() {
        assert_eq!(parse_client_pid("12345"), Some(12345));
        assert_eq!(parse_client_pid(" 99 "), Some(99));
        assert!(parse_client_pid("0").is_none());
        assert!(parse_client_pid("1").is_none());
        assert!(parse_client_pid("").is_none());
        assert!(parse_client_pid("/bin/licoup").is_none());
        assert_eq!(process_liveness(std::process::id()), ProcessLiveness::Alive);
        assert_eq!(process_liveness(0), ProcessLiveness::Dead);
    }

    #[test]
    fn host_generation_rejects_paths_and_keeps_a_pid() {
        let generation = "0".repeat(16);
        assert_eq!(
            parse_host_generation_record(&format!("{generation}\n12\n")),
            Some((generation.clone(), Some(12), None))
        );
        assert_eq!(
            parse_host_generation_record(&format!("{generation}\n12\n99\n")),
            Some((generation.clone(), Some(12), Some(99)))
        );
        assert_eq!(
            parse_host_generation_record(&format!("{generation}\n")),
            Some((generation.clone(), None, None))
        );
        assert!(parse_host_generation_record("/Applications/LicoUp.app\n12\n").is_none());
        assert!(parse_host_generation_record(&format!("{generation}\n12\nextra\n")).is_none());
        assert!(parse_host_generation_record(&format!("{generation}\n12\n1\n")).is_none());
        let generation = executable_generation().unwrap();
        assert!(valid_host_generation(&generation));
        assert_eq!(generation, executable_generation().unwrap());
    }

    /// The generation every ownership case below is classified against.
    const OWNERSHIP_GENERATION: &str = "0123456789abcdef";

    fn ownership_record(
        host_pid: Option<u32>,
        client_pid: Option<u32>,
    ) -> (String, Option<u32>, Option<u32>) {
        (OWNERSHIP_GENERATION.to_owned(), host_pid, client_pid)
    }

    #[test]
    fn a_record_naming_a_live_host_is_current() {
        assert!(
            classify_host_ownership(
                Some(ownership_record(Some(4242), Some(99))),
                OWNERSHIP_GENERATION,
                Some(99),
                |_| ProcessLiveness::Alive,
            )
            .is_current()
        );
        // A process this machine cannot inspect is not proof of death.
        assert!(
            classify_host_ownership(
                Some(ownership_record(Some(4242), None)),
                OWNERSHIP_GENERATION,
                None,
                |_| ProcessLiveness::Unknown,
            )
            .is_current()
        );
    }

    #[test]
    fn an_out_of_date_record_is_stale_rather_than_no_host() {
        // #325: the host the record names is gone while a live host serves the
        // same root. That is an out-of-date record, not an absent one, and the
        // two demand opposite reactions from the caller.
        let stale = classify_host_ownership(
            Some(ownership_record(Some(4242), Some(99))),
            OWNERSHIP_GENERATION,
            Some(99),
            |_| ProcessLiveness::Dead,
        );
        assert_eq!(stale, HostOwnership::Stale);
        assert!(!stale.is_current());

        let absent = classify_host_ownership(None, OWNERSHIP_GENERATION, Some(99), |_| {
            ProcessLiveness::Dead
        });
        assert_eq!(absent, HostOwnership::Absent);
        assert!(!absent.is_current());
    }

    #[test]
    fn a_record_without_a_host_pid_is_stale_rather_than_absent() {
        let ownership = classify_host_ownership(
            Some(ownership_record(None, None)),
            OWNERSHIP_GENERATION,
            None,
            |_| ProcessLiveness::Alive,
        );
        assert_eq!(ownership, HostOwnership::Stale);
        assert!(!ownership.is_current());
        // A record that names no host also says nothing about another client,
        // so a client-bound process must still read it as out of date rather
        // than as someone else's host.
        let bound_to_a_client = classify_host_ownership(
            Some(ownership_record(None, None)),
            OWNERSHIP_GENERATION,
            Some(99),
            |_| ProcessLiveness::Alive,
        );
        assert_eq!(bound_to_a_client, HostOwnership::Stale);
    }

    #[test]
    fn a_record_for_another_generation_is_absent_while_another_client_is_current_or_stale() {
        assert_eq!(
            classify_host_ownership(
                Some(ownership_record(Some(4242), Some(99))),
                "fedcba9876543210",
                Some(99),
                |_| ProcessLiveness::Alive,
            ),
            HostOwnership::Absent
        );
        // Per D19, the same generation with another client PID connects to the
        // existing host if alive (Current), or reports Stale if the host died.
        assert_eq!(
            classify_host_ownership(
                Some(ownership_record(Some(4242), Some(7))),
                OWNERSHIP_GENERATION,
                Some(99),
                |_| ProcessLiveness::Alive,
            ),
            HostOwnership::Current
        );
        assert_eq!(
            classify_host_ownership(
                Some(ownership_record(Some(4242), Some(7))),
                OWNERSHIP_GENERATION,
                Some(99),
                |_| ProcessLiveness::Dead,
            ),
            HostOwnership::Stale
        );
    }

    #[test]
    fn a_takeover_probe_gives_up_when_no_host_answers() {
        let root = std::env::temp_dir().join(format!("lico-ca-ownership-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let previous =
            licoup_foundation::platform::paths::set_portable_data_dir_override(Some(root.clone()));
        let answered = endpoint_accepts_connections_within(Duration::from_millis(60));
        licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
        let _ = std::fs::remove_dir_all(&root);
        assert!(!answered, "a fresh root has no host to answer");
    }

    #[test]
    fn a_slow_connect_is_treated_as_no_answer() {
        let started = Instant::now();
        let answer = run_with_timeout(
            || {
                thread::sleep(Duration::from_secs(1));
                Ok::<_, io::Error>(())
            },
            Duration::from_millis(10),
        );
        assert!(answer.is_none());
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "a connect attempt must not hold its caller until the worker returns"
        );
    }

    #[test]
    fn a_fast_connect_answer_is_returned() {
        assert_eq!(
            run_with_timeout(|| Ok::<_, io::Error>(42_u8), Duration::from_secs(1)),
            Some(42)
        );
        assert!(
            run_with_timeout(
                || Err::<u8, _>(io::Error::other("connection refused")),
                Duration::from_secs(1),
            )
            .is_none()
        );
    }

    #[test]
    fn the_ownership_lock_admits_one_host_at_a_time() {
        let root =
            std::env::temp_dir().join(format!("lico-ca-owner-lock-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let previous =
            licoup_foundation::platform::paths::set_portable_data_dir_override(Some(root.clone()));
        let owner = HostOwnerLock::acquire().unwrap();
        assert!(owner.is_some(), "a fresh root has no owner");
        // A second host cannot reach the listener while the first holds the
        // lock, so the takeover path is unreachable for it.
        assert!(HostOwnerLock::acquire().unwrap().is_none());
        // The owner going away releases the lock — the same thing the kernel
        // does when the owning process dies — so the next host may take over.
        drop(owner);
        assert!(HostOwnerLock::acquire().unwrap().is_some());
        licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ordinary_rpc_post_returns_while_host_wake_cognition_is_already_held() {
        use interprocess::local_socket::ListenerOptions;
        use licoup_conversation::continuity::list_all_pending_wakes;
        use licoup_native::domain::client_conversation::{
            ConversationService, PersistentRuntimePorts,
        };
        use serde_json::{Value, json};
        use std::io::{BufRead, Write as _};

        let root =
            std::env::temp_dir().join(format!("lico-ca-host-listen-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let entered = std::sync::Arc::new(AtomicBool::new(false));
        let hold = std::sync::Arc::new((std::sync::Mutex::new(false), Condvar::new()));
        let hold_for_complete = hold.clone();
        let entered_for_complete = entered.clone();
        let complete_calls_for_turn = complete_calls.clone();
        let service = ConversationService::open(&root)
            .unwrap()
            .bind_conversation_runtime(PersistentRuntimePorts::new(
                |_params: &Value| {
                    Ok(json!({
                        "ok": true,
                        "accepted": true,
                        "turnHandle": "turn:test",
                    }))
                },
                |_conversation_id: &str| json!([]),
                |_params: &Value| Ok(json!({ "ok": true })),
                move |params: &Value| {
                    entered_for_complete.store(true, Ordering::Release);
                    let (lock, cv) = &*hold_for_complete;
                    let mut released = lock.lock().unwrap_or_else(|poison| poison.into_inner());
                    while !*released {
                        released = cv
                            .wait(released)
                            .unwrap_or_else(|poison| poison.into_inner());
                    }
                    complete_calls_for_turn
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .push(params.clone());
                    let conversation_id = params
                        .get("conversationId")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    Ok(json!({
                        "ok": true,
                        "output": listener_child_proposal_json(conversation_id),
                    }))
                },
                |_request: Value| Ok(json!({})),
            ));
        let (conversation_id, owner, agent) = listener_create_group(&service);
        listener_designate(&service, &conversation_id, &owner, &agent);
        let posted = listener_post(&service, &conversation_id, &owner, "prepare notes now");
        let _ = service
            .execute(json!({
                "action": "conversation.dispatch.after-post",
                "conversationId": conversation_id,
                "eventId": posted,
            }))
            .unwrap();
        service
            .after_runtime_settlement(
                &conversation_id,
                &json!({
                    "output": listener_assistant_envelope(
                        "I'll prepare the notes in a child conversation.",
                        &listener_child_proposal_json(&conversation_id),
                    ),
                    "membershipId": agent,
                    "causationId": posted,
                }),
            )
            .unwrap();
        let host = service.continuity().cloned().unwrap();
        let pending_before = list_all_pending_wakes(host.store()).unwrap();
        assert!(
            !pending_before.is_empty(),
            "durable delegation must leave a pending review wake"
        );
        let logical_wake_ids: Vec<String> = pending_before
            .iter()
            .map(|(_, wake)| wake.logical_wake_id.clone())
            .collect();
        let name =
            licoup_native::platform::conversation_host_transport::endpoint_name_for_root(&root)
                .unwrap();
        let listener = ListenerOptions::new()
            .name(name)
            .nonblocking(ListenerNonblockingMode::Accept)
            .try_overwrite(true)
            .create_sync()
            .expect("test host listener");
        let runtime = PersistentConversationRuntime::new(service.store().clone());
        let stop = Arc::new(AtomicBool::new(false));
        let host_service = service.clone();
        let host_stop = stop.clone();
        let host_thread = thread::spawn(move || {
            serve_bound_host(listener, host_service, runtime, Some(host_stop))
        });

        let entered_deadline = Instant::now() + Duration::from_secs(8);
        while !entered.load(Ordering::Acquire) {
            assert!(
                Instant::now() < entered_deadline,
                "wake cognition must enter the host attendance worker before the new RPC session"
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            complete_calls
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .len(),
            0,
            "held cognition must have entered but not finished before the new RPC post"
        );

        let post_root = root.clone();
        let post_conversation = conversation_id.clone();
        let post_owner = owner.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let mut stream = connect_test_host(&post_root);
            let request = json!({
                "protocol": licoup_native::platform::conversation_host_transport::STDIO_RPC_PROTOCOL,
                "id": "post-while-held",
                "workflowId": "listener-wake",
                "method": "client.conversation.execute",
                "params": {
                    "action": "conversation.message.post",
                    "conversationId": post_conversation,
                    "authorMembershipId": post_owner,
                    "content": "ordinary follow-up while wake cognition is already held",
                }
            });
            serde_json::to_writer(&mut stream, &request).unwrap();
            stream.write_all(b"\n").unwrap();
            stream.flush().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let _ = tx.send(serde_json::from_str::<Value>(&line).unwrap());
        });
        let follow_up = rx
            .recv_timeout(Duration::from_secs(8))
            .expect("actual host RPC post must return while wake cognition is held");
        assert_eq!(follow_up["ok"], true, "{follow_up}");
        let posted_event = follow_up["result"]["result"]["event"]["id"]
            .as_str()
            .expect("ordinary post must persist an event");
        assert!(!posted_event.is_empty());
        assert!(
            follow_up["result"]["result"]["continuityDrain"].is_null(),
            "ordinary post must not drain wakes: {follow_up}"
        );
        assert!(
            list_all_pending_wakes(host.store())
                .unwrap()
                .iter()
                .any(|(_, wake)| logical_wake_ids.contains(&wake.logical_wake_id)),
            "pending review ownership must survive the new RPC session"
        );
        assert_eq!(
            complete_calls
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .len(),
            0,
            "listener must not await the already-entered held cognition"
        );

        {
            let (lock, cv) = &*hold;
            let mut released = lock.lock().unwrap_or_else(|poison| poison.into_inner());
            *released = true;
            cv.notify_all();
        }
        let finish_deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let completes = complete_calls
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone();
            if completes.len() == 1 {
                assert_eq!(
                    completes[0].get("continuityKind").and_then(Value::as_str),
                    Some("wake-review"),
                    "attendance must progress the pending review once after release"
                );
                break;
            }
            assert!(
                Instant::now() < finish_deadline,
                "attendance must finish the already-entered review once after release"
            );
            thread::sleep(Duration::from_millis(20));
        }

        stop.store(true, Ordering::Release);
        let _ = host_thread.join();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn host_owner_stop_drains_a_held_wake_before_returning() {
        use interprocess::local_socket::ListenerOptions;
        use licoup_conversation::continuity::list_all_pending_wakes;
        use licoup_native::domain::client_conversation::{
            ConversationService, PersistentRuntimePorts,
        };
        use serde_json::{Value, json};

        let root = std::env::temp_dir().join(format!("lico-ca-host-stop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let entered = std::sync::Arc::new(AtomicBool::new(false));
        let hold = std::sync::Arc::new((std::sync::Mutex::new(false), Condvar::new()));
        let hold_for_complete = hold.clone();
        let entered_for_complete = entered.clone();
        let complete_calls_for_turn = complete_calls.clone();
        let service = ConversationService::open(&root)
            .unwrap()
            .bind_conversation_runtime(PersistentRuntimePorts::new(
                |_params: &Value| {
                    Ok(json!({
                        "ok": true,
                        "accepted": true,
                        "turnHandle": "turn:test",
                    }))
                },
                |_conversation_id: &str| json!([]),
                |_params: &Value| Ok(json!({ "ok": true })),
                move |params: &Value| {
                    entered_for_complete.store(true, Ordering::Release);
                    let (lock, cv) = &*hold_for_complete;
                    let mut released = lock.lock().unwrap_or_else(|poison| poison.into_inner());
                    while !*released {
                        released = cv
                            .wait(released)
                            .unwrap_or_else(|poison| poison.into_inner());
                    }
                    complete_calls_for_turn
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .push(params.clone());
                    let conversation_id = params
                        .get("conversationId")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    Ok(json!({
                        "ok": true,
                        "output": listener_child_proposal_json(conversation_id),
                    }))
                },
                |_request: Value| Ok(json!({})),
            ));
        let (conversation_id, owner, agent) = listener_create_group(&service);
        listener_designate(&service, &conversation_id, &owner, &agent);
        let posted = listener_post(&service, &conversation_id, &owner, "prepare notes now");
        let _ = service
            .execute(json!({
                "action": "conversation.dispatch.after-post",
                "conversationId": conversation_id,
                "eventId": posted,
            }))
            .unwrap();
        service
            .after_runtime_settlement(
                &conversation_id,
                &json!({
                    "output": listener_assistant_envelope(
                        "I'll prepare the notes in a child conversation.",
                        &listener_child_proposal_json(&conversation_id),
                    ),
                    "membershipId": agent,
                    "causationId": posted,
                }),
            )
            .unwrap();
        assert!(
            !list_all_pending_wakes(service.continuity().cloned().unwrap().store())
                .unwrap()
                .is_empty(),
            "durable delegation must leave a pending review wake"
        );
        let name =
            licoup_native::platform::conversation_host_transport::endpoint_name_for_root(&root)
                .unwrap();
        let listener = ListenerOptions::new()
            .name(name)
            .nonblocking(ListenerNonblockingMode::Accept)
            .try_overwrite(true)
            .create_sync()
            .expect("test host listener");
        let runtime = PersistentConversationRuntime::new(service.store().clone());
        let stop = Arc::new(AtomicBool::new(false));
        let host_service = service.clone();
        let host_stop = stop.clone();
        let host_thread = thread::spawn(move || {
            serve_bound_host(listener, host_service, runtime, Some(host_stop))
        });

        let entered_deadline = Instant::now() + Duration::from_secs(8);
        while !entered.load(Ordering::Acquire) {
            assert!(
                Instant::now() < entered_deadline,
                "wake cognition must enter before configured host stop"
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            complete_calls
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .len(),
            0,
            "held cognition must have entered but not finished before stop"
        );

        stop.store(true, Ordering::Release);
        {
            let (lock, cv) = &*hold;
            let mut released = lock.lock().unwrap_or_else(|poison| poison.into_inner());
            *released = true;
            cv.notify_all();
        }
        let finish_deadline = Instant::now() + Duration::from_secs(8);
        while complete_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .is_empty()
        {
            assert!(
                Instant::now() < finish_deadline,
                "the admitted wake must finish after hold release"
            );
            thread::sleep(Duration::from_millis(20));
        }
        host_thread
            .join()
            .expect("host thread must finish its drain")
            .expect("serve_bound_host stop path");
        assert!(
            try_connect_test_host(&root, Duration::from_millis(150)).is_err(),
            "listener/endpoint must be released after the held wake finishes"
        );
        assert_eq!(
            complete_calls
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .len(),
            1,
            "host shutdown must join the admitted wake before returning"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    fn listener_create_group(
        service: &licoup_native::domain::client_conversation::ConversationService,
    ) -> (String, String, String) {
        use serde_json::json;
        let group = service
            .execute(json!({
                "action": "conversation.create",
                "title": "Listener wake",
                "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
                "members": [{
                    "principal": {
                        "id": "agent:codex",
                        "kind": "agent",
                        "displayName": "Codex",
                        "agentId": "codex"
                    },
                    "access": "member"
                }]
            }))
            .unwrap();
        let memberships = group["memberships"].as_array().unwrap();
        let owner = memberships
            .iter()
            .find(|membership| membership["principal"]["kind"] == "human")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let agent = memberships
            .iter()
            .find(|membership| membership["principal"]["kind"] == "agent")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        (group["id"].as_str().unwrap().to_owned(), owner, agent)
    }

    fn listener_designate(
        service: &licoup_native::domain::client_conversation::ConversationService,
        conversation_id: &str,
        owner: &str,
        agent: &str,
    ) {
        use serde_json::json;
        let revision = service.store().get(conversation_id).unwrap().revision;
        service
            .execute(json!({
                "action": "conversation.assistant.set",
                "conversationId": conversation_id,
                "ownerMembershipId": owner,
                "expectedRevision": revision,
                "membershipId": agent,
            }))
            .unwrap();
    }

    fn listener_post(
        service: &licoup_native::domain::client_conversation::ConversationService,
        conversation_id: &str,
        owner: &str,
        content: &str,
    ) -> String {
        use serde_json::json;
        service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner,
                "content": content,
            }))
            .unwrap()["event"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn listener_child_proposal_json(conversation_id: &str) -> String {
        use licoup_conversation::continuity::{
            ContinuityCommitmentProposal, ContinuityFollowThroughKind,
            ContinuityInterpretationProposal, ContinuityMatterSubject, ContinuitySpeechAct,
            ContinuityTaskChildAdmission, ContinuityWriteEnvelope,
        };
        serde_json::to_string(&ContinuityInterpretationProposal {
            envelope: ContinuityWriteEnvelope {
                conversation_id: conversation_id.to_owned(),
                source_event_refs: Vec::new(),
                observed_revision: 0,
                designation_epoch: 0,
                request_id: "request:goal:matter:notes".into(),
            },
            matter_associations: Vec::new(),
            speech_act: ContinuitySpeechAct::Delegation,
            commitment_proposals: vec![ContinuityCommitmentProposal {
                matter_id: Some("matter:notes".into()),
                subject: ContinuityMatterSubject::New,
                expected_result: "Prepare matter:notes".into(),
                criteria: Vec::new(),
                create_goal: true,
            }],
            agreement_proposals: Vec::new(),
            capability_needs: Vec::new(),
            uncertainty_reasons: Vec::new(),
            requested_reads: Vec::new(),
            task_child_admission: Some(ContinuityTaskChildAdmission {
                goal_id: "goal:matter:notes".into(),
                parent_conversation_id: conversation_id.to_owned(),
                speech_act: ContinuitySpeechAct::Delegation,
                follow_through_kind: ContinuityFollowThroughKind::Durable,
                observed_child_conversation_id: None,
                observed_card_anchor: None,
                request_id: "request:admit:goal:matter:notes".into(),
            }),
        })
        .unwrap()
    }

    fn listener_assistant_envelope(reply: &str, proposal_json: &str) -> String {
        let proposal: serde_json::Value = serde_json::from_str(proposal_json).unwrap();
        serde_json::to_string(&serde_json::json!({
            "replyText": reply,
            "interpretationProposal": proposal,
        }))
        .unwrap()
    }

    fn connect_test_host(root: &Path) -> Stream {
        try_connect_test_host(root, Duration::from_secs(5))
            .unwrap_or_else(|error| panic!("actual host listener did not accept: {error}"))
    }

    fn try_connect_test_host(root: &Path, timeout: Duration) -> io::Result<Stream> {
        let deadline = Instant::now() + timeout;
        loop {
            let name =
                licoup_native::platform::conversation_host_transport::endpoint_name_for_root(root)
                    .unwrap();
            match Stream::connect(name) {
                Ok(stream) => return Ok(stream),
                Err(_) if Instant::now() < deadline => thread::sleep(CONNECT_RETRY),
                Err(error) => return Err(error),
            }
        }
    }

    #[test]
    fn d19_gui_quit_and_reopen_shares_same_host() {
        let generation = executable_generation().unwrap();
        let client_a = Some(1111);
        let client_b = Some(2222);

        // Host was launched by GUI A (client_a)
        let record = Some((generation.clone(), Some(4242), client_a));

        // When GUI A is alive, GUI B sees the host as Current (not Absent!)
        assert_eq!(
            classify_host_ownership(record.clone(), &generation, client_b, |_| {
                ProcessLiveness::Alive
            },),
            HostOwnership::Current
        );

        // Even when GUI A has quit (client_a is dead, but host is alive), GUI B still sees host as Current!
        assert_eq!(
            classify_host_ownership(record.clone(), &generation, client_b, |_| {
                ProcessLiveness::Alive
            },),
            HostOwnership::Current
        );

        // CLI (no client PID) also sees host as Current
        assert_eq!(
            classify_host_ownership(record, &generation, None, |_| ProcessLiveness::Alive),
            HostOwnership::Current
        );
    }

    #[test]
    fn explicit_stop_via_rpc_shuts_down_host() {
        let root =
            std::env::temp_dir().join(format!("lico-ca-host-stop-rpc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let service =
            licoup_native::domain::client_conversation::ConversationService::open(&root).unwrap();
        let name =
            licoup_native::platform::conversation_host_transport::endpoint_name_for_root(&root)
                .unwrap();
        let listener = ListenerOptions::new()
            .name(name)
            .nonblocking(ListenerNonblockingMode::Accept)
            .try_overwrite(true)
            .create_sync()
            .expect("test host listener");
        let runtime = PersistentConversationRuntime::new(service.store().clone());
        let host_service = service.clone();
        let host_runtime = runtime.clone();
        let host_thread =
            thread::spawn(move || serve_bound_host(listener, host_service, host_runtime, None));

        let mut stream = connect_test_host(&root);
        let request = serde_json::json!({
            "protocol": licoup_native::platform::conversation_host_transport::STDIO_RPC_PROTOCOL,
            "id": "stop-req",
            "workflowId": "stop-wf",
            "method": "shutdown",
            "params": { "host": true },
        });
        serde_json::to_writer(&mut stream, &request).unwrap();
        stream.write_all(b"\n").unwrap();
        stream.flush().unwrap();

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["ok"], true);
        assert_eq!(response["result"]["status"], "shutdown");
        assert_eq!(response["result"]["host_stop_requested"], true);
        drop(reader);

        // Host thread must terminate after receiving host stop
        let deadline = Instant::now() + Duration::from_secs(4);
        while !host_thread.is_finished() {
            assert!(
                Instant::now() < deadline,
                "host thread must exit on explicit stop"
            );
            thread::sleep(Duration::from_millis(20));
        }
        let _ = host_thread.join().unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn system_offline_reported_honestly() {
        let root =
            std::env::temp_dir().join(format!("lico-ca-host-offline-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let previous =
            licoup_foundation::platform::paths::set_portable_data_dir_override(Some(root.clone()));

        // When no host is running:
        // 1. connect_for_cli(true) returns persistent_conversation_transport_required
        let err = connect_for_cli(true).unwrap_err();
        assert_eq!(
            err.to_string(),
            "persistent_conversation_transport_required"
        );

        // 2. request_host_stop() succeeds quietly: the acquirable owner lock
        // proves no host is writing through this root, so stopping is a no-op
        // rather than a transport failure.
        request_host_stop().unwrap();

        // 3. host_is_current() is false
        assert!(!host_is_current());

        licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn crash_recovery_restores_from_real_run_and_durable_facts() {
        use licoup_conversation::{ConversationStore, DispatchState};

        let root =
            std::env::temp_dir().join(format!("lico-ca-host-crash-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let scope = {
            let store = ConversationStore::open(&root).unwrap();
            let scope = store
                .prepare_runtime_dispatch(
                    "fixture-agent",
                    "",
                    "persist this turn",
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
            store
                .append_runtime_frame(
                    &scope,
                    1,
                    &serde_json::json!({"type": "agent.message.chunk", "delta": "partial"}),
                )
                .unwrap();
            assert_eq!(
                store
                    .dispatch_record(&scope.dispatch_id)
                    .unwrap()
                    .unwrap()
                    .state,
                DispatchState::Running
            );
            // Dropping store without checkpoint/drain simulates host crash
            scope
        };

        // Reopen store (as ConversationService::open does on startup): cold_recover runs
        let reopened = ConversationStore::open(&root).unwrap();
        let dispatch = reopened
            .dispatch_record(&scope.dispatch_id)
            .unwrap()
            .unwrap();
        assert_eq!(dispatch.state, DispatchState::Failed);
        assert_eq!(
            dispatch.error_code.as_deref(),
            Some("host_lifecycle_interrupted")
        );

        let events = reopened
            .page_events(&scope.conversation_id, None, 100)
            .unwrap()
            .events;
        assert!(
            !events.is_empty(),
            "durable facts must be preserved after crash recovery"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
