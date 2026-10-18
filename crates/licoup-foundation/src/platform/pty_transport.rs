//! Shared pseudo-terminal foundation for CLI agent lanes.
//!
//! Agent CLIs such as Antigravity's `agy --print` behave differently on a
//! pipe: `isatty` probes fail, TUI rendering degrades, and output may only be
//! observable after the process exits. Attaching the child's stdin and stdout
//! to a pseudo-terminal slave gives real TTY semantics while the master fd
//! stays in this process, so output can be read incrementally.
//!
//! Design constraints:
//! - Zero new dependencies: raw `libc` FFI (`openpty`, `cfmakeraw`,
//!   `tcgetattr`/`tcsetattr`, `TIOCSWINSZ`) on Unix only.
//! - The slave runs in raw mode (OPOST off), so `\n` is not translated to
//!   `\r\n` and structured line-based protocols (Cursor NDJSON) parse
//!   byte-identically to pipes.
//! - stderr is intentionally left as a real pipe by callers: driver stderr
//!   counting semantics stay intact and structured stdout parsing never sees
//!   stderr noise.
//! - No controlling terminal is created (`setsid`/`TIOCSCTTY` deferred to a
//!   future interactive-TUI lane); `isatty` plus streaming is all the current
//!   `--print` lanes need.

use crate::platform::process_supervisor::SupervisedChild;
use crate::platform::raw_execution::{RawExecutionDirection, RawExecutionObserver};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

/// Default terminal size applied to the pty before spawn. `--print` lanes do
/// not reflow, but a sane winsize keeps TUI-aware CLIs from mis-rendering.
const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 30;

/// Spawns `command` with stdin and stdout attached to a raw pty slave.
///
/// Takes the `Command` by value and drops it before returning: `std` keeps
/// `Stdio::from(OwnedFd)` descriptors open in the parent until the `Command`
/// drops, and a parent-held slave fd would keep the master from reaching
/// EOF/EIO when the child exits — every turn would stall until timeout.
/// Callers set `.stderr(...)` themselves before passing the command.
pub fn spawn(command: Command) -> io::Result<(SupervisedChild, Master)> {
    let (master, slave) = open_pty()?;
    make_slave_raw(slave.as_raw_fd())?;
    set_winsize(master.as_raw_fd(), DEFAULT_COLS, DEFAULT_ROWS)?;
    set_cloexec(master.as_raw_fd())?;
    set_cloexec(slave.as_raw_fd())?;
    let mut command = command;
    command
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave));
    let child = SupervisedChild::spawn(&mut command)?;
    drop(command);
    Ok((
        child,
        Master {
            fd: master,
            raw_observer: RawExecutionObserver::current(),
        },
    ))
}

/// Blocking byte handle on the pty master fd.
///
/// `Read` translates Linux's EIO (all slave fds closed) into a clean EOF so
/// natural child exits close the stream instead of surfacing as read errors,
/// and retries EINTR (child exit can deliver SIGCHLD).
pub struct Master {
    fd: OwnedFd,
    raw_observer: Option<RawExecutionObserver>,
}

impl Read for Master {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let count = unsafe {
                libc::read(
                    self.fd.as_raw_fd(),
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                )
            };
            if count > 0 {
                if let Some(observer) = self.raw_observer.as_ref() {
                    observer.record_bytes(
                        "pty",
                        RawExecutionDirection::Received,
                        &buf[..count as usize],
                    );
                }
                return Ok(count as usize);
            }
            if count == 0 {
                return Ok(0);
            }
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::EIO) => return Ok(0),
                _ => return Err(error),
            }
        }
    }
}

impl Write for Master {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        loop {
            let count = unsafe {
                libc::write(
                    self.fd.as_raw_fd(),
                    buf.as_ptr() as *const libc::c_void,
                    buf.len(),
                )
            };
            if count >= 0 {
                if count > 0
                    && let Some(observer) = self.raw_observer.as_ref()
                {
                    observer.record_bytes(
                        "pty",
                        RawExecutionDirection::Sent,
                        &buf[..count as usize],
                    );
                }
                return Ok(count as usize);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(error);
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Master {
    // Foundation API for the future interactive-TUI lane: a writer handle
    // independent of the reader thread, and terminal reflow. The current
    // `--print` lanes need neither, so they are kept available rather than
    // wired into callers that would never exercise them.
    #[allow(dead_code)]
    pub fn try_clone(&self) -> io::Result<Self> {
        let fd = unsafe { libc::dup(self.fd.as_raw_fd()) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
            raw_observer: self.raw_observer.clone(),
        })
    }

    #[allow(dead_code)]
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        set_winsize(self.fd.as_raw_fd(), cols, rows)
    }
}

/// Reader-thread protocol. Ordering guarantee: at most one `Truncated`, it
/// always follows the final delivered `Data`, and no `Data` arrives after it.
#[derive(Debug)]
pub enum PtyEvent {
    Data(Vec<u8>),
    Truncated,
    Closed,
}

/// Body for `thread::spawn(move || pty_transport::read_master(master, sender, max_bytes))`.
///
/// `max_bytes = None` is unbounded (cursor lane). On exceeding the cap the
/// allowed prefix is delivered as `Data`, then `Truncated`, and reads continue
/// DISCARDING until EOF so a chatty child can never block on the pty buffer —
/// this is what makes truncate-and-succeed reliable. Always ends with `Closed`.
pub fn read_master(mut master: Master, sender: Sender<PtyEvent>, max_bytes: Option<usize>) {
    let mut buffer = [0u8; 8192];
    let mut remaining = max_bytes;
    loop {
        match master.read(&mut buffer) {
            Ok(0) => {
                let _ = sender.send(PtyEvent::Closed);
                return;
            }
            Ok(count) => match remaining {
                None => {
                    let _ = sender.send(PtyEvent::Data(buffer[..count].to_vec()));
                }
                Some(0) => {} // discard after the cap was exceeded
                Some(left) if count > left => {
                    let _ = sender.send(PtyEvent::Data(buffer[..left].to_vec()));
                    remaining = Some(0);
                    let _ = sender.send(PtyEvent::Truncated);
                }
                Some(left) => {
                    let _ = sender.send(PtyEvent::Data(buffer[..count].to_vec()));
                    remaining = Some(left - count);
                }
            },
            Err(_) => {
                let _ = sender.send(PtyEvent::Closed);
                return;
            }
        }
    }
}

fn open_pty() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((unsafe { OwnedFd::from_raw_fd(master) }, unsafe {
        OwnedFd::from_raw_fd(slave)
    }))
}

fn make_slave_raw(fd: RawFd) -> io::Result<()> {
    unsafe {
        let mut termios = std::mem::zeroed::<libc::termios>();
        if libc::tcgetattr(fd, &mut termios) != 0 {
            return Err(io::Error::last_os_error());
        }
        // Clears OPOST (\n -> \r\n), ICANON, ECHO, ISIG on the slave.
        libc::cfmakeraw(&mut termios);
        if libc::tcsetattr(fd, libc::TCSANOW, &termios) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn set_winsize(fd: RawFd, cols: u16, rows: u16) -> io::Result<()> {
    let winsize = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &winsize) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn set_cloexec(fd: RawFd) -> io::Result<()> {
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::platform::process_supervisor::{IO_THREAD_EXIT_GRACE, join_bounded};
    use std::process::Command;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    fn collect_events(receiver: mpsc::Receiver<PtyEvent>, timeout: Duration) -> Vec<PtyEvent> {
        let deadline = Instant::now() + timeout;
        let mut events = Vec::new();
        while Instant::now() < deadline {
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(PtyEvent::Closed) => {
                    events.push(PtyEvent::Closed);
                    return events;
                }
                Ok(event) => events.push(event),
                Err(_) => {}
            }
        }
        panic!("pty event stream did not close within {timeout:?}");
    }

    fn data_text(events: &[PtyEvent]) -> String {
        let mut text = String::new();
        for event in events {
            if let PtyEvent::Data(bytes) = event {
                text.push_str(&String::from_utf8_lossy(bytes));
            }
        }
        text
    }

    fn spawn_sh(script: &str) -> (SupervisedChild, Master) {
        let mut command = Command::new("sh");
        command.arg("-c").arg(script).stderr(Stdio::null());
        spawn(command).expect("pty spawn failed")
    }

    #[test]
    fn spawn_streams_data_incrementally_and_closes() {
        let (mut child, master) = spawn_sh("printf 'first\\n'; sleep 0.3; printf 'second\\n'");
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || read_master(master, sender, None));
        let events = collect_events(receiver, Duration::from_secs(5));
        let data_events = events
            .iter()
            .filter(|event| matches!(event, PtyEvent::Data(_)))
            .count();
        assert!(
            data_events >= 2,
            "expected incremental delivery: {events:?}"
        );
        assert_eq!(data_text(&events), "first\nsecond\n");
        assert!(matches!(events.last(), Some(PtyEvent::Closed)));
        join_bounded(handle, IO_THREAD_EXIT_GRACE).expect("reader join");
        let status = child
            .terminate_tree()
            .expect("terminate")
            .expect("exit status");
        assert!(status.success());
    }

    #[test]
    fn byte_cap_truncates_and_still_reaches_closed() {
        let records = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&records);
        let observer = RawExecutionObserver::new(move |_, _, raw| {
            sink.lock().unwrap().push(raw.to_owned());
            Ok(())
        });
        let _scope = crate::platform::raw_execution::RawExecutionScope::enter(Some(observer));
        let (mut child, master) = spawn_sh("printf 'hello world\\n'; sleep 0.2; printf 'tail\\n'");
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || read_master(master, sender, Some(5)));
        let events = collect_events(receiver, Duration::from_secs(5));
        assert_eq!(data_text(&events), "hello");
        assert!(
            events
                .iter()
                .filter(|event| matches!(event, PtyEvent::Truncated))
                .count()
                == 1
        );
        assert!(matches!(events.last(), Some(PtyEvent::Closed)));
        join_bounded(handle, IO_THREAD_EXIT_GRACE).expect("reader join");
        let status = child
            .terminate_tree()
            .expect("terminate")
            .expect("exit status");
        assert!(
            status.success(),
            "drain-and-discard must not block natural exit"
        );
        assert_eq!(records.lock().unwrap().concat(), "hello world\ntail\n");
    }

    #[test]
    fn slave_close_yields_eof_not_error() {
        let (_child, master) = spawn_sh("exit 0");
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || read_master(master, sender, None));
        let events = collect_events(receiver, Duration::from_secs(5));
        assert!(matches!(events.last(), Some(PtyEvent::Closed)));
        join_bounded(handle, IO_THREAD_EXIT_GRACE).expect("reader join");
    }

    #[test]
    fn terminate_tree_unblocks_master_read() {
        // Establish the long-lived descendant before reporting readiness.
        // Printing first left a race where termination could land between the
        // shell write and its `sleep` spawn, so the test was not actually
        // observing a stable process tree under parallel load.
        let (mut child, master) =
            spawn_sh("sleep 30 & sleeper=$!; printf ready; wait \"$sleeper\"");
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || read_master(master, sender, None));
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut terminated = false;
        while Instant::now() < deadline {
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(PtyEvent::Data(bytes)) if String::from_utf8_lossy(&bytes).contains("ready") => {
                    child.terminate_tree().expect("terminate");
                    terminated = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(terminated, "child never reported readiness");
        // Fresh deadline: terminate_tree's own exit grace must not consume the
        // budget of the close wait (shared deadlines flake under parallel load).
        let close_deadline = Instant::now() + Duration::from_secs(5);
        let mut closed = false;
        while Instant::now() < close_deadline {
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(PtyEvent::Closed) => {
                    closed = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(closed, "master read must unblock after terminate_tree");
        join_bounded(handle, IO_THREAD_EXIT_GRACE).expect("reader join");
    }

    #[test]
    fn write_bytes_reaches_child_stdin() {
        let (mut child, master) = spawn_sh("read line; printf 'got:%s\\n' \"$line\"");
        let (sender, receiver) = mpsc::channel();
        // Clone the master before moving the original into the reader thread so
        // a writer handle survives independently.
        let mut writer = master.try_clone().expect("clone");
        let handle = thread::spawn(move || read_master(master, sender, None));
        writer.write_all(b"hello\n").expect("write");
        let events = collect_events(receiver, Duration::from_secs(5));
        assert_eq!(data_text(&events), "got:hello\n");
        assert!(matches!(events.last(), Some(PtyEvent::Closed)));
        join_bounded(handle, IO_THREAD_EXIT_GRACE).expect("reader join");
        child.terminate_tree().expect("terminate");
    }

    #[test]
    fn spawn_missing_executable_fails() {
        let mut command = Command::new("/definitely/missing/pty-fake");
        command.stderr(Stdio::null());
        assert!(spawn(command).is_err());
    }

    #[test]
    fn spawn_sets_default_winsize() {
        // `stty size` reads the winsize of the terminal on its stdin, which is
        // the pty slave; our spawn applies 120x30 before the child executes.
        let (mut child, master) = spawn_sh("stty size");
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || read_master(master, sender, None));
        let events = collect_events(receiver, Duration::from_secs(5));
        assert!(
            data_text(&events).contains("30 120"),
            "expected default 120x30: {:?}",
            data_text(&events)
        );
        join_bounded(handle, IO_THREAD_EXIT_GRACE).expect("reader join");
        child.terminate_tree().expect("terminate");
    }
}
