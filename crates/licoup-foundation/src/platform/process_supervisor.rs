use command_group::{CommandGroup, GroupChild};
use std::env;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const PROCESS_EXIT_GRACE: Duration = Duration::from_secs(2);
const PROCESS_GRACEFUL_EXIT_GRACE: Duration = Duration::from_millis(100);
pub const IO_THREAD_EXIT_GRACE: Duration = Duration::from_secs(1);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);
const STDIN_QUEUE_CAPACITY: usize = 8;

#[derive(Debug)]
pub struct BoundedCommandOutput {
    pub status: Option<ExitStatus>,
    pub stdout: Vec<u8>,
    pub timed_out: bool,
    pub truncated: bool,
}

/// Clear inherited process state before launching a third-party Agent binary.
///
/// A child that is not a user-approved app identity attributes TCC prompts to
/// LicoUp. Dropping the inherited cwd and environment shrinks that blast
/// radius; it does not replace skipping the spawn on unused-agent discovery.
pub fn configure_untrusted_agent_command(command: &mut Command) {
    let inherited = minimal_untrusted_agent_env();
    command.env_clear();
    for (key, value) in inherited {
        command.env(key, value);
    }
    let workdir = untrusted_agent_workdir();
    let _ = std::fs::create_dir_all(&workdir);
    command.current_dir(workdir);
    command.stdin(std::process::Stdio::null());
}

fn minimal_untrusted_agent_env() -> Vec<(OsString, OsString)> {
    const KEYS: &[&str] = &[
        "PATH",
        "PATHEXT",
        "SYSTEMROOT",
        "WINDIR",
        "ComSpec",
        "TMPDIR",
        "TMP",
        "TEMP",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
    ];
    let mut pairs = Vec::new();
    for key in KEYS {
        if let Some(value) = env::var_os(key).filter(|value| !value.is_empty()) {
            pairs.push((OsString::from(*key), value));
        }
    }
    if let Some(home) = crate::platform::paths::user_home_from_env() {
        let home = crate::platform::paths::strip_macos_data_volume(&home);
        let home_value = home.into_os_string();
        pairs.push((OsString::from("HOME"), home_value.clone()));
        if cfg!(windows) {
            pairs.push((OsString::from("USERPROFILE"), home_value));
        }
    }
    pairs.push((OsString::from("TERM"), OsString::from("dumb")));
    pairs.push((OsString::from("NO_COLOR"), OsString::from("1")));
    pairs
}

fn untrusted_agent_workdir() -> PathBuf {
    env::temp_dir().join("licoup-agent-probe")
}

/// Bounded output for an untrusted Agent CLI: isolated env, pinned cwd, null stdin.
pub fn run_bounded_untrusted_agent_output(
    command: &mut Command,
    timeout: Duration,
    max_output: usize,
) -> io::Result<BoundedCommandOutput> {
    configure_untrusted_agent_command(command);
    run_bounded_command_output(command, timeout, max_output)
}

/// Runs a batch command with a hard deadline, bounded captured output, and
/// process-tree cleanup. This is intended for optional local capability probes
/// whose executables are outside LicoUp's control.
pub fn run_bounded_command_output(
    command: &mut Command,
    timeout: Duration,
    max_output: usize,
) -> io::Result<BoundedCommandOutput> {
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = SupervisedChild::spawn(command)?;
    let stdout = child
        .stdout()
        .ok_or_else(|| io::Error::other("supervised stdout unavailable"))?;
    let stderr = child
        .stderr()
        .ok_or_else(|| io::Error::other("supervised stderr unavailable"))?;
    let stdout_handle = thread::spawn(move || read_bounded_bytes(stdout, max_output));
    let stderr_handle = thread::spawn(move || read_bounded_bytes(stderr, max_output));
    let deadline = Instant::now() + timeout;
    while (!stdout_handle.is_finished() || !stderr_handle.is_finished())
        && Instant::now() < deadline
    {
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
    let timed_out = !stdout_handle.is_finished() || !stderr_handle.is_finished();
    let status = child
        .terminate_tree()
        .map_err(|_| io::Error::other("supervised process cleanup failed"))?;
    let stdout = join_bounded(stdout_handle, IO_THREAD_EXIT_GRACE)
        .map_err(|_| io::Error::other("supervised stdout cleanup failed"))??;
    let stderr = join_bounded(stderr_handle, IO_THREAD_EXIT_GRACE)
        .map_err(|_| io::Error::other("supervised stderr cleanup failed"))??;
    Ok(BoundedCommandOutput {
        status,
        stdout: stdout.bytes,
        timed_out,
        truncated: stdout.truncated || stderr.truncated,
    })
}

/// Runs a sandboxed child with one bounded stdin document and bounded output.
pub fn run_bounded_command_input(
    command: &mut Command,
    input: &[u8],
    timeout: Duration,
    max_output: usize,
) -> io::Result<BoundedCommandOutput> {
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = SupervisedChild::spawn(command)?;
    let mut stdin = child
        .stdin()
        .ok_or_else(|| io::Error::other("supervised stdin unavailable"))?;
    stdin.write_all(input)?;
    drop(stdin);
    let stdout = child
        .stdout()
        .ok_or_else(|| io::Error::other("supervised stdout unavailable"))?;
    let stderr = child
        .stderr()
        .ok_or_else(|| io::Error::other("supervised stderr unavailable"))?;
    let stdout_handle = thread::spawn(move || read_bounded_bytes(stdout, max_output));
    let stderr_handle = thread::spawn(move || read_bounded_bytes(stderr, max_output));
    let deadline = Instant::now() + timeout;
    while (!stdout_handle.is_finished() || !stderr_handle.is_finished())
        && Instant::now() < deadline
    {
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
    let timed_out = !stdout_handle.is_finished() || !stderr_handle.is_finished();
    let status = child
        .terminate_tree()
        .map_err(|_| io::Error::other("supervised process cleanup failed"))?;
    let stdout = join_bounded(stdout_handle, IO_THREAD_EXIT_GRACE)
        .map_err(|_| io::Error::other("supervised stdout cleanup failed"))??;
    let stderr = join_bounded(stderr_handle, IO_THREAD_EXIT_GRACE)
        .map_err(|_| io::Error::other("supervised stderr cleanup failed"))??;
    Ok(BoundedCommandOutput {
        status,
        stdout: stdout.bytes,
        timed_out,
        truncated: stdout.truncated || stderr.truncated,
    })
}

struct BoundedBytes {
    bytes: Vec<u8>,
    truncated: bool,
}

fn read_bounded_bytes(reader: impl Read, max_output: usize) -> io::Result<BoundedBytes> {
    let read_limit = u64::try_from(max_output)
        .unwrap_or(u64::MAX - 1)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(max_output.min(8192));
    reader.take(read_limit).read_to_end(&mut bytes)?;
    let truncated = bytes.len() > max_output;
    bytes.truncate(max_output);
    Ok(BoundedBytes { bytes, truncated })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleFailure {
    Terminate,
    Wait,
    IoThread,
}

#[derive(Debug)]
pub struct SupervisedChild {
    child: GroupChild,
    cleaned: bool,
    pid: u32,
}

impl SupervisedChild {
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        #[cfg(windows)]
        let mut child = command.group().kill_on_drop(true).spawn()?;
        #[cfg(not(windows))]
        let mut child = command.group_spawn()?;
        let pid = child.inner().id();

        Ok(Self {
            child,
            cleaned: false,
            pid,
        })
    }

    pub fn stdin(&mut self) -> Option<ChildStdin> {
        self.child.inner().stdin.take()
    }

    pub fn stdout(&mut self) -> Option<ChildStdout> {
        self.child.inner().stdout.take()
    }

    pub fn stderr(&mut self) -> Option<ChildStderr> {
        self.child.inner().stderr.take()
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.cleaned = true;
                Ok(Some(status))
            }
            other => other,
        }
    }

    /// Waits for the supervised process to exit without imposing a cleanup
    /// grace. On Unix, the leader is observed with WNOWAIT so its process-group
    /// ID remains reserved until the caller removes its cancellation entry and
    /// calls `terminate_tree`. On Windows, the job handle remains the stable
    /// cleanup identity, so the existing group wait is sufficient.
    pub fn wait_for_natural_exit(
        &mut self,
        deadline: Option<Instant>,
    ) -> Result<bool, LifecycleFailure> {
        if self.cleaned {
            return self
                .child
                .try_wait()
                .map(|status| status.is_some())
                .map_err(|_| LifecycleFailure::Wait);
        }

        #[cfg(unix)]
        loop {
            let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.pid as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result != 0 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(LifecycleFailure::Wait);
            }
            if unsafe { info.si_pid() } != 0 {
                return Ok(true);
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Ok(false);
            }
            thread::sleep(PROCESS_POLL_INTERVAL);
        }

        #[cfg(not(unix))]
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    self.cleaned = true;
                    return Ok(true);
                }
                Ok(None) => {
                    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                        return Ok(false);
                    }
                    thread::sleep(PROCESS_POLL_INTERVAL);
                }
                Err(_) => return Err(LifecycleFailure::Wait),
            }
        }
    }

    /// Gives a batch-style child a bounded opportunity to report its natural
    /// exit status before terminating any process tree it left behind. The
    /// root stays unreaped during the grace period so its process-group ID
    /// cannot be recycled before descendant cleanup.
    pub fn finish_or_terminate_tree(
        &mut self,
        graceful_exit_grace: Duration,
    ) -> Result<Option<ExitStatus>, LifecycleFailure> {
        if self.cleaned {
            return self.child.try_wait().map_err(|_| LifecycleFailure::Wait);
        }

        let deadline = Instant::now() + graceful_exit_grace;
        while Instant::now() < deadline {
            thread::sleep(PROCESS_POLL_INTERVAL);
        }
        self.terminate_tree()
    }

    /// Terminates the complete supervised process tree and reaps the root.
    ///
    /// This must be called on every non-panicking path. `Drop` is only a final
    /// leak-prevention fallback because it cannot report lifecycle errors.
    pub fn terminate_tree(&mut self) -> Result<Option<ExitStatus>, LifecycleFailure> {
        if self.cleaned {
            return self.child.try_wait().map_err(|_| LifecycleFailure::Wait);
        }

        self.child
            .kill()
            .or_else(|error| {
                if process_group_is_gone(&error) {
                    Ok(())
                } else {
                    Err(error)
                }
            })
            .map_err(|_| LifecycleFailure::Terminate)?;

        let status = wait_for_exit(&mut self.child, PROCESS_EXIT_GRACE)?;
        self.cleaned = true;
        Ok(status)
    }

    fn terminate_best_effort(&mut self) {
        if self.cleaned {
            return;
        }
        let _ = self.child.kill();
        self.cleaned = true;
    }
}

/// The observed end of one owned process group after a control-lane
/// termination. `Unconfirmed` is never reported as an exit: a signal that was
/// delivered without an observed exit stays unconfirmed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnedGroupExit {
    ObservedExit { forced: bool, already_exited: bool },
    Unconfirmed,
}

/// One LicoUp-owned detached execution group, identified by the group leader
/// pid LicoUp recorded when it spawned the process detached. Control-lane
/// force stop terminates exactly this group; verification refuses a pid that
/// does not lead its own group, so a shared or external process is never
/// widened into the signal.
#[derive(Clone, Copy, Debug)]
pub struct OwnedProcessGroup {
    pid: u32,
}

impl OwnedProcessGroup {
    /// Verify the ownership record before any signal. The pid must be live and
    /// must be the leader of its own process group.
    pub fn verify(pid: u32) -> Option<Self> {
        let group = Self { pid };
        (pid != 0 && group.leads_own_group()).then_some(group)
    }

    fn leads_own_group(&self) -> bool {
        if !owned_process_alive(self.pid) {
            return false;
        }
        #[cfg(unix)]
        {
            // SAFETY: getpgid only queries the process table.
            let group = unsafe { libc::getpgid(self.pid as libc::pid_t) };
            group > 0 && group as u32 == self.pid
        }
        #[cfg(not(unix))]
        {
            // Windows creates the detached group through the spawn flags; the
            // durable pid record is the ownership proof and the tree kill is
            // bounded to that pid.
            true
        }
    }

    /// Terminate this owned group with a bounded graceful window, then a
    /// bounded forced window, and report only what was observed.
    pub fn terminate_and_observe(&self, bound: Duration) -> OwnedGroupExit {
        if !self.leads_own_group() {
            return OwnedGroupExit::ObservedExit {
                forced: false,
                already_exited: true,
            };
        }
        let graceful = self.signal_group(false);
        if self.await_exit(bound) {
            return OwnedGroupExit::ObservedExit {
                forced: false,
                already_exited: false,
            };
        }
        if graceful {
            let _ = self.signal_group(true);
        } else {
            // A refused graceful signal must not turn into a silent forced one.
            return OwnedGroupExit::Unconfirmed;
        }
        if self.await_exit(bound) {
            OwnedGroupExit::ObservedExit {
                forced: true,
                already_exited: false,
            }
        } else {
            OwnedGroupExit::Unconfirmed
        }
    }

    fn await_exit(&self, bound: Duration) -> bool {
        let deadline = Instant::now() + bound;
        loop {
            if !self.leads_own_group() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(PROCESS_POLL_INTERVAL);
        }
    }

    /// Signal the whole group, never a single member: `-pid` addresses the
    /// group LicoUp created at spawn.
    fn signal_group(&self, force: bool) -> bool {
        #[cfg(unix)]
        {
            let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
            // SAFETY: killpg with a negative pid addresses the verified group.
            unsafe { libc::kill(-(self.pid as libc::pid_t), signal) == 0 }
        }
        #[cfg(windows)]
        {
            let mut command = Command::new("taskkill");
            command
                .args(["/PID", &self.pid.to_string(), "/T"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            if force {
                command.arg("/F");
            }
            command
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
        }
    }
}

fn owned_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // A zombie still answers kill(pid, 0); for lifecycle purposes it has
        // already exited and its group no longer runs work.
        if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0
            && io::Error::last_os_error().raw_os_error() != Some(libc::EPERM)
        {
            return false;
        }
        !owned_process_is_zombie(pid)
    }
    #[cfg(windows)]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid)])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .map(|output| {
                output.status.success()
                    && output.stdout.len() <= 64 * 1024
                    && String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
            })
            .unwrap_or(false)
    }
}

#[cfg(target_os = "macos")]
fn owned_process_is_zombie(pid: u32) -> bool {
    // XNU drops zombies from proc_pidinfo's view while they still answer
    // kill(pid, 0), so a successful kill followed by ESRCH here means zombie.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let written = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    written <= 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

#[cfg(target_os = "linux")]
fn owned_process_is_zombie(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().next())
        == Some("Z")
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn owned_process_is_zombie(_pid: u32) -> bool {
    false
}

fn process_group_is_gone(error: &io::Error) -> bool {
    if matches!(
        error.kind(),
        io::ErrorKind::InvalidInput | io::ErrorKind::NotFound
    ) {
        return true;
    }
    #[cfg(unix)]
    if matches!(
        error.raw_os_error(),
        Some(code)
            if code == nix::errno::Errno::ESRCH as i32
                || code == nix::errno::Errno::EPERM as i32
    ) {
        // Darwin reports EPERM when a process group contains only the
        // unreaped zombie leader. The leader is deliberately left unreaped
        // until after this signal attempt, so its process-group ID cannot have
        // been recycled. A live same-credential descendant remains signalable
        // and therefore does not take this branch.
        return true;
    }
    false
}

impl Drop for SupervisedChild {
    fn drop(&mut self) {
        self.terminate_best_effort();
    }
}

fn wait_for_exit(
    child: &mut GroupChild,
    grace: Duration,
) -> Result<Option<ExitStatus>, LifecycleFailure> {
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(Some(status)),
            Ok(None) if Instant::now() < deadline => thread::sleep(PROCESS_POLL_INTERVAL),
            Ok(None) => return Err(LifecycleFailure::Wait),
            Err(_) => return Err(LifecycleFailure::Wait),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StdinFailure {
    Unavailable,
    Busy,
    Write,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportFinishFailure {
    Lifecycle,
    StdinWrite,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WriterEvent {
    Written,
    Failed,
}

#[derive(Debug)]
pub struct BoundedStdinWriter {
    sender: Option<SyncSender<Vec<u8>>>,
    events: Receiver<WriterEvent>,
    handle: Option<JoinHandle<Result<(), StdinFailure>>>,
    failed: bool,
}

impl BoundedStdinWriter {
    pub fn new(mut stdin: ChildStdin) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(STDIN_QUEUE_CAPACITY);
        let (event_sender, events) = mpsc::channel();
        let handle = thread::spawn(move || {
            while let Ok(bytes) = receiver.recv() {
                if stdin.write_all(&bytes).is_err() || stdin.flush().is_err() {
                    let _ = event_sender.send(WriterEvent::Failed);
                    return Err(StdinFailure::Write);
                }
                let _ = event_sender.send(WriterEvent::Written);
            }
            drop(stdin);
            Ok(())
        });
        Self {
            sender: Some(sender),
            events,
            handle: Some(handle),
            failed: false,
        }
    }

    pub fn enqueue(&mut self, bytes: Vec<u8>) -> Result<(), StdinFailure> {
        self.check_health()?;
        let Some(sender) = self.sender.as_ref() else {
            return Err(StdinFailure::Unavailable);
        };
        match sender.try_send(bytes) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(StdinFailure::Busy),
            Err(TrySendError::Disconnected(_)) => {
                self.failed = true;
                Err(StdinFailure::Write)
            }
        }
    }

    pub fn check_health(&mut self) -> Result<(), StdinFailure> {
        if self.failed {
            return Err(StdinFailure::Write);
        }
        loop {
            match self.events.try_recv() {
                Ok(WriterEvent::Written) => {}
                Ok(WriterEvent::Failed) => {
                    self.failed = true;
                    return Err(StdinFailure::Write);
                }
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => {
                    if self
                        .handle
                        .as_ref()
                        .is_some_and(|handle| handle.is_finished())
                    {
                        return Ok(());
                    }
                    self.failed = true;
                    return Err(StdinFailure::Write);
                }
            }
        }
    }

    pub fn finish(&mut self, grace: Duration) -> Result<(), StdinFailure> {
        self.sender.take();
        let Some(handle) = self.handle.take() else {
            return if self.failed {
                Err(StdinFailure::Write)
            } else {
                Ok(())
            };
        };
        match join_bounded(handle, grace) {
            Ok(Ok(())) if !self.failed => Ok(()),
            Ok(Ok(())) | Ok(Err(_)) => Err(StdinFailure::Write),
            Err(_) => Err(StdinFailure::Shutdown),
        }
    }
}

pub fn join_bounded<T>(handle: JoinHandle<T>, grace: Duration) -> Result<T, LifecycleFailure> {
    let deadline = Instant::now() + grace;
    while !handle.is_finished() && Instant::now() < deadline {
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
    if !handle.is_finished() {
        drop(handle);
        return Err(LifecycleFailure::IoThread);
    }
    handle.join().map_err(|_| LifecycleFailure::IoThread)
}

pub fn finish_protocol_transport(
    child: &mut SupervisedChild,
    stdin: &mut BoundedStdinWriter,
    stdout_handle: JoinHandle<()>,
    stderr_handle: JoinHandle<()>,
) -> Result<(), TransportFinishFailure> {
    let stdin = stdin.finish(IO_THREAD_EXIT_GRACE);
    let graceful_deadline = Instant::now() + PROCESS_GRACEFUL_EXIT_GRACE;
    while !stdout_handle.is_finished() && Instant::now() < graceful_deadline {
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
    let process = child.terminate_tree();
    let stdout = join_bounded(stdout_handle, IO_THREAD_EXIT_GRACE);
    let stderr = join_bounded(stderr_handle, IO_THREAD_EXIT_GRACE);
    if process.is_err()
        || stdout.is_err()
        || stderr.is_err()
        || stdin == Err(StdinFailure::Shutdown)
    {
        return Err(TransportFinishFailure::Lifecycle);
    }
    if stdin.is_err() {
        return Err(TransportFinishFailure::StdinWrite);
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Read;
    use std::process::Stdio;

    #[test]
    fn termination_closes_pipe_retained_by_descendant() {
        let mut command = Command::new("sh");
        command
            .args(["-c", "sleep 30 & printf ready"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = SupervisedChild::spawn(&mut command).unwrap();
        let mut stdout = child.stdout().unwrap();
        let reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).map(|_| bytes)
        });

        thread::sleep(Duration::from_millis(50));
        child.terminate_tree().unwrap();
        let bytes = join_bounded(reader, IO_THREAD_EXIT_GRACE).unwrap().unwrap();
        assert_eq!(bytes, b"ready");
    }

    #[test]
    fn blocked_stdin_writer_does_not_own_the_deadline_loop() {
        let mut command = Command::new("sh");
        command
            .args(["-c", "sleep 30"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = SupervisedChild::spawn(&mut command).unwrap();
        let stdin = child.stdin().unwrap();
        let mut writer = BoundedStdinWriter::new(stdin);
        let started = Instant::now();
        writer.enqueue(vec![b'x'; 16 * 1024 * 1024]).unwrap();

        thread::sleep(Duration::from_millis(50));
        child.terminate_tree().unwrap();
        writer.finish(IO_THREAD_EXIT_GRACE).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn timeout_terminates_root_and_pipe_holding_descendant() {
        let mut command = Command::new("sh");
        command
            .args(["-c", "sleep 30 & printf ready; sleep 30"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = SupervisedChild::spawn(&mut command).unwrap();
        let mut stdout = child.stdout().unwrap();
        let reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).map(|_| bytes)
        });
        let started = Instant::now();
        let deadline = started + Duration::from_millis(50);
        while Instant::now() < deadline {
            thread::sleep(PROCESS_POLL_INTERVAL);
        }

        child.terminate_tree().unwrap();
        let bytes = join_bounded(reader, IO_THREAD_EXIT_GRACE).unwrap().unwrap();
        assert_eq!(bytes, b"ready");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn bounded_grace_preserves_a_natural_batch_exit_status() {
        let mut command = Command::new("sh");
        command
            .args(["-c", "exec 1>&-; sleep 0.05; exit 0"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = SupervisedChild::spawn(&mut command).unwrap();
        let mut stdout = child.stdout().unwrap();
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).unwrap();

        let status = child
            .finish_or_terminate_tree(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn cleanup_reaps_a_naturally_exited_group() {
        let mut command = Command::new("sh");
        command
            .args(["-c", "exit 0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = SupervisedChild::spawn(&mut command).unwrap();
        thread::sleep(Duration::from_millis(50));
        let status = child.terminate_tree().unwrap().unwrap();
        assert!(status.success());
    }

    #[cfg(unix)]
    #[test]
    fn untrusted_agent_command_pins_cwd() {
        let mut command = Command::new("pwd");
        configure_untrusted_agent_command(&mut command);
        command.stdout(Stdio::piped()).stderr(Stdio::null());
        let output = command.output().unwrap();
        assert!(output.status.success());
        let cwd = String::from_utf8_lossy(&output.stdout);
        assert!(cwd.contains("licoup-agent-probe"), "{cwd}");
    }
}
