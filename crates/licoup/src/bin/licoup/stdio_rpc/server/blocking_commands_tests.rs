use super::*;
use std::io::Cursor;
use std::sync::{Condvar, mpsc};
use std::time::Duration;

struct FrameWriter {
    partial: Vec<u8>,
    frames: mpsc::Sender<Value>,
}

impl Write for FrameWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        for &byte in bytes {
            if byte == b'\n' {
                let frame = serde_json::from_slice(&self.partial)?;
                self.frames.send(frame).map_err(io::Error::other)?;
                self.partial.clear();
            } else {
                self.partial.push(byte);
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn input(requests: &[Value]) -> Cursor<Vec<u8>> {
    let mut bytes = Vec::new();
    for request in requests {
        serde_json::to_writer(&mut bytes, request).unwrap();
        bytes.push(b'\n');
    }
    Cursor::new(bytes)
}

fn execute_request(id: &str, args: &[&str]) -> Value {
    json!({
        "protocol": STDIO_RPC_PROTOCOL, "id": id, "workflowId": "synthetic",
        "method": "execute", "args": args,
    })
}

#[test]
fn blocked_scan_and_key_migration_leave_the_same_rpc_frame_loop_responsive() {
    for mut request in [
        json!({
            "protocol": STDIO_RPC_PROTOCOL, "id": "blocked", "workflowId": "synthetic",
            "method": "targets.scan", "params": {"targetIds": ["codex"], "modelCatalogTargetIds": ["codex"]},
        }),
        execute_request("blocked", &["llm-gateway", "credentials", "migrate"]),
    ] {
        let root = std::env::temp_dir().join("licoup-rpc-synthetic-root");
        request["portableDataDir"] = json!(root);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_gate = Arc::clone(&gate);
        let (started, observed_start) = mpsc::channel();
        let (frames, observed_frames) = mpsc::channel();
        let worker_executor: Arc<Executor> = Arc::new(move |command| {
            assert!(matches!(
                command.path(),
                ["targets", "scan"] | ["llm-gateway", "credentials", "migrate"]
            ));
            assert_eq!(
                licoup_foundation::platform::paths::portable_data_dir_override_path(),
                Some(root.clone())
            );
            started.send(()).unwrap();
            let (lock, changed) = &*worker_gate;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = changed.wait(released).unwrap();
            }
            Ok(CliExecution::Json(json!({"status": "completed"})))
        });
        let fallback = Arc::clone(&worker_executor);
        let server = std::thread::spawn(move || {
            super::super::serve_stdio_rpc_inner(
                input(&[request, execute_request("light", &["synthetic-light"])]),
                FrameWriter {
                    partial: Vec::new(),
                    frames,
                },
                move |args, root| {
                    if args[0] != "synthetic-light" {
                        let _guard = PortableDataDirOverrideGuard::set(root);
                        return fallback(admit_cli_command(args)?);
                    }
                    assert!(
                        licoup_foundation::platform::paths::portable_data_dir_override_path()
                            .is_none()
                    );
                    Ok(CliExecution::Json(json!({"status": "responsive"})))
                },
                None,
                None,
                false,
                Workers {
                    handles: Vec::new(),
                    execute: worker_executor,
                },
            )
        });
        // Test-only bounds detect a blocked server without leaving its gate
        // held on failure. Production workers have no wait deadline.
        let started = observed_start.recv_timeout(Duration::from_secs(5));
        let first = observed_frames.recv_timeout(Duration::from_secs(5));
        let (lock, changed) = &*gate;
        *lock.lock().unwrap() = true;
        changed.notify_all();
        server.join().unwrap().unwrap();
        assert!(started.is_ok(), "the blocking command must actually start");
        let first = first.expect("a later frame must respond while the gate is held");
        assert_eq!(first["id"], "light");
        assert_eq!(first["result"]["status"], "responsive");
        let completed = observed_frames.recv().unwrap();
        assert_eq!(completed["id"], "blocked");
        assert_eq!(completed["result"]["status"], "completed");
    }
}

#[test]
fn blocking_command_admission_is_exact_and_rejects_invalid_arguments() {
    let args = |values: &[&str]| {
        values
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>()
    };
    assert!(admit(&args(&["targets", "add"])).unwrap().is_none());
    assert!(
        admit(&args(&["llm-gateway", "credentials", "create"]))
            .unwrap()
            .is_none()
    );
    assert!(
        admit(&args(&[
            "llm-gateway",
            "credentials",
            "migrate",
            "unexpected"
        ]))
        .is_err()
    );
    assert!(admit(&args(&["targets", "scan", "--unexpected", "true"])).is_err());
    assert!(admit(&args(&["targets", "scan", "--stdin-json", "not-json"])).is_err());
}

#[test]
fn ordinary_commands_stay_on_the_ordered_lane() {
    let owner = std::thread::current().id();
    let output = super::super::serve_stdio_rpc_inner(
        input(&[execute_request(
            "ordered",
            &["llm-gateway", "credentials", "list"],
        )]),
        Vec::new(),
        |_, _| {
            assert_eq!(std::thread::current().id(), owner);
            Ok(CliExecution::Json(json!({"status": "ordered"})))
        },
        None,
        None,
        false,
        Workers {
            handles: Vec::new(),
            execute: Arc::new(|_| panic!("unrelated command reached worker")),
        },
    )
    .unwrap();
    let frame: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(frame["result"]["status"], "ordered");
}
