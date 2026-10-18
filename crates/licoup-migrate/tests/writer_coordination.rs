//! The shipped tool shares the client's real process-lifetime data-home lease.

mod support;

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use support::*;

const HELPER: &str = "LICOUP_TEST_MIGRATION_WRITER";
const READY: &str = "MIGRATION_WRITER_READY";

struct Writer(Child);

impl Writer {
    fn start(home: &Path) -> Self {
        let mut child = Self(
            isolated_command(std::env::current_exe().expect("test binary"), home)
                .args(["--exact", "selected_home_writer_helper", "--nocapture"])
                .env(HELPER, "hold")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("start owned writer"),
        );
        let stdout = child.0.stdout.take().expect("writer output");
        let (ready_tx, ready_rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if line.is_ok_and(|line| line.trim() == READY) {
                    let _ = ready_tx.send(());
                }
            }
        });
        ready_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("writer acquired its actual process lease");
        child
    }

    fn release(mut self) {
        self.0
            .stdin
            .take()
            .expect("writer input")
            .write_all(b"release\n")
            .expect("release owned writer");
        assert!(self.0.wait().expect("writer exit").success());
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        // Clean up only this test's child, including on an assertion failure.
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

#[test]
fn selected_home_writer_helper() {
    if std::env::var(HELPER).as_deref() != Ok("hold") {
        return;
    }
    let _lease = licoup_foundation::platform::data_home_access::acquire_process_data_home_access()
        .expect("selected-home process lease");
    println!("{READY}");
    std::io::stdout().flush().expect("ready signal");
    let mut release = String::new();
    std::io::stdin()
        .read_line(&mut release)
        .expect("release signal");
    assert_eq!(release.trim(), "release");
}

#[test]
fn mutating_verbs_refuse_a_live_writer_without_touching_roots_and_resume_after_release() {
    let fixture = TestRoot::new("writer-coordination");
    let home = fixture.join("home");
    let source = fixture.join("source");
    let archive = fixture.join("backup.zip");
    let target = fixture.join("target");
    let work = fixture.join("work");
    write_file(&source, "opaque.txt", b"synthetic state");
    save_data_home_locator(&home, &source);
    let before = root_files(&source);
    let mut writer = Writer::start(&home);
    let source_arg = source.to_str().unwrap();
    let archive_arg = archive.to_str().unwrap();
    let target_arg = target.to_str().unwrap();
    let work_arg = work.to_str().unwrap();

    for args in [
        vec!["convert", "--data-root", source_arg, "--writers-stopped"],
        vec!["resume", "--data-root", source_arg, "--writers-stopped"],
        vec![
            "export",
            "--data-root",
            source_arg,
            "--archive",
            archive_arg,
            "--writers-stopped",
        ],
        vec![
            "import",
            "--archive",
            archive_arg,
            "--target-root",
            target_arg,
        ],
        vec![
            "rehearse",
            "--data-root",
            source_arg,
            "--work-root",
            work_arg,
            "--writers-stopped",
        ],
    ] {
        let (code, report) = run_tool_in_home(&home, &args);
        assert_eq!(code, 1, "{}: {report}", args[0]);
        assert_eq!(report["error"], "backup_writers_running", "{}", args[0]);
        assert_eq!(root_files(&source), before);
        assert!(!archive.exists());
        assert!(!target.exists());
        assert!(!work.exists());
        assert!(
            writer.0.try_wait().unwrap().is_none(),
            "writer was not stopped"
        );
    }
    // Observation does not need exclusive mutation authority and stays read-only.
    for verb in ["inspect", "plan"] {
        let (code, report) = run_tool_in_home(&home, &[verb, "--data-root", source_arg]);
        assert_eq!(code, 0, "{verb}: {report}");
        assert_eq!(root_files(&source), before);
    }
    writer.release();
    let (code, report) = run_tool_in_home(
        &home,
        &[
            "export",
            "--data-root",
            source_arg,
            "--archive",
            archive_arg,
            "--writers-stopped",
        ],
    );
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["status"], "exported");
    assert_eq!(root_files(&source), before);
    let (code, report) = run_tool_in_home(
        &home,
        &[
            "import",
            "--archive",
            archive_arg,
            "--target-root",
            target_arg,
        ],
    );
    assert_eq!(code, 0, "{report}");
    assert_eq!(
        std::fs::read(target.join("opaque.txt")).unwrap(),
        b"synthetic state"
    );
}
