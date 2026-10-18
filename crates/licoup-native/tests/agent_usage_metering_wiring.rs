//! The metering port on the client's real command path.
//!
//! `UsageJournal::open` is only worth having if a production entry calls it.
//! These cases run the real `licoup-cli` binary — the boundary the desktop
//! client drives — over a synthetic data home, and they read the result from
//! two sides that do not share an in-process cache:
//!
//! - what the process wrote: the ledger store under the selected state root,
//!   with the fact rows the port settled;
//! - what the next process reports: `agent-usage report` projects that ledger
//!   through the fact port's `read`, so a journal that was never composed is
//!   visible as `available: false` and as an unsettled scan.
//!
//! Removing the scan's composition call leaves the settle without a host: the
//! scan reports `metering: null` and the store keeps zero settled fact rows, so
//! these cases fail on the missing facts themselves rather than on a weakened
//! assertion. The store file still appears, because the workflow-ledger
//! projection of the same verb reads the same store; the facts are what the
//! metering port owns.

use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const LEDGER_STORE: &str = "agent-usage/graph-usage-ledger-v2.sqlite3";
const FACT_ROWS: &str = "SELECT COUNT(*) FROM usage_source_facts";

fn synthetic_root(label: &str) -> PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "lico-agent-usage-metering-{label}-{nonce}-{}",
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).expect("synthetic root");
    path
}

/// The fallback timestamp a synthetic reading carries: today, so the case
/// cannot fall out of the scan's history window as the clock moves.
fn today_utc() -> String {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86_400;
    // Howard Hinnant's civil-from-days conversion, so the case needs no clock
    // crate beyond `SystemTime`.
    let shifted = days as i64 + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

/// One Codex rollout holding exactly one counted reading: pooled totals and the
/// per-turn delta agree, so the parsed reading is reported rather than estimated.
fn codex_rollout(codex_home: &Path) {
    let sessions = codex_home.join("sessions");
    fs::create_dir_all(&sessions).expect("synthetic codex home");
    let total = json!({"input_tokens": 120, "cached_input_tokens": 20, "output_tokens": 30});
    let last = total.clone();
    let day = today_utc();
    let timestamp = format!("{day}T10:00:02Z");
    fs::write(
        sessions.join("rollout-synthetic.jsonl"),
        [
            json!({
                "timestamp": format!("{day}T10:00:00Z"),
                "type": "session_meta",
                "payload": {"id": "session-metering-wiring"}
            })
            .to_string(),
            json!({
                "timestamp": format!("{day}T10:00:01Z"),
                "type": "turn_context",
                "payload": {"model": "gpt-synthetic"}
            })
            .to_string(),
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": total["input_tokens"],
                            "cached_input_tokens": total["cached_input_tokens"],
                            "output_tokens": total["output_tokens"],
                            "reasoning_output_tokens": 15,
                            "total_tokens": 150
                        },
                        "last_token_usage": {
                            "input_tokens": last["input_tokens"],
                            "cached_input_tokens": last["cached_input_tokens"],
                            "output_tokens": last["output_tokens"],
                            "reasoning_output_tokens": 15,
                            "total_tokens": 150
                        }
                    }
                }
            })
            .to_string(),
        ]
        .join("\n"),
    )
    .expect("synthetic rollout");
}

/// Run the real client binary with an isolated home, exactly as the desktop
/// client drives it.
fn run_cli(home: &Path, codex_home: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_licoup-cli"));
    command
        .args(args)
        .env("HOME", home)
        .env("CODEX_HOME", codex_home)
        .env_remove("LICOUP_HOME")
        .env_remove("LICOUP_PORTABLE_DIR")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("RUST_LOG")
        .env_remove("RUST_BACKTRACE");
    command
        .output()
        .expect("the real licoup binary must be runnable")
}

fn run_json(home: &Path, codex_home: &Path, args: &[&str]) -> Value {
    let output = run_cli(home, codex_home, args);
    assert!(
        output.status.success(),
        "`{}` must succeed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "`{}` must report JSON ({error}): {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

/// The settled fact rows, read from the store by an observer that shares no
/// in-process state with the process that wrote them.
fn settled_fact_rows(state_root: &Path) -> i64 {
    let ledger = state_root.join(LEDGER_STORE);
    assert!(
        ledger.is_file(),
        "the scan must settle into {}",
        ledger.display()
    );
    let connection = Connection::open_with_flags(&ledger, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("the ledger store the scan composed");
    connection
        .query_row(FACT_ROWS, [], |row| row.get(0))
        .expect("the base fact table the journal owns")
}

#[test]
fn a_real_scan_command_settles_the_readings_it_took_and_the_report_projects_them() {
    let home = synthetic_root("home");
    let state = synthetic_root("state");
    let codex_home = synthetic_root("codex");
    codex_rollout(&codex_home);

    let scanned = run_json(
        &home,
        &codex_home,
        &[
            "agent-usage",
            "scan",
            "--agent",
            "codex",
            "--state-root",
            &state.display().to_string(),
            "--force-refresh",
        ],
    );
    let codex = scanned["agents"]
        .as_array()
        .expect("the scan reports its agents")
        .iter()
        .find(|agent| agent["agentId"] == "codex")
        .expect("the scan reports the agent it was asked for");
    assert_eq!(
        codex["history"]["totalTokens"].as_u64(),
        Some(150),
        "the production scan read the synthetic reading: {codex}"
    );
    let metering = &codex["metering"];
    assert_eq!(
        metering["inserted"].as_u64(),
        Some(4),
        "one counted reading settles one fact per token metric: {metering}"
    );
    assert_eq!(metering["replayed"].as_u64(), Some(0));

    // The port's durable side, read from the store rather than from the report
    // the same process composed.
    assert_eq!(settled_fact_rows(&state), 4);

    // A second command in a second process is a replay, not a second fact.
    let rescanned = run_json(
        &home,
        &codex_home,
        &[
            "agent-usage",
            "scan",
            "--agent",
            "codex",
            "--state-root",
            &state.display().to_string(),
            "--force-refresh",
        ],
    );
    let codex = rescanned["agents"]
        .as_array()
        .expect("the rescan reports its agents")
        .iter()
        .find(|agent| agent["agentId"] == "codex")
        .expect("the rescan reports the agent it was asked for");
    assert_eq!(
        codex["metering"]["replayed"].as_u64(),
        Some(4),
        "a rescan of the same numbers replays: {}",
        codex["metering"]
    );
    assert_eq!(settled_fact_rows(&state), 4, "a replay adds no fact row");

    let reported = run_json(
        &home,
        &codex_home,
        &[
            "agent-usage",
            "report",
            "--state-root",
            &state.display().to_string(),
        ],
    );
    // `available: false` is the unreadable-store marker; its absence says the
    // projection is the ledger's own answer rather than a refusal.
    assert!(
        reported["metering"].get("available").is_none(),
        "the report read the ledger the scan composed: {}",
        reported["metering"]
    );
    assert_eq!(reported["metering"]["journal"], "usage-journal.v1");
    assert_eq!(
        reported["metering"]["facts"].as_u64(),
        Some(4),
        "the report reads the base facts through the fact port: {}",
        reported["metering"]
    );

    let _ = fs::remove_dir_all(&home);
    let _ = fs::remove_dir_all(&state);
    let _ = fs::remove_dir_all(&codex_home);
}

/// The projection is the ledger's own answer, not a claim: a home whose scan
/// never settled a reading reports an available store with no facts, and it
/// names the journal and the store it read so a zero can be told apart from an
/// unreadable ledger.
#[test]
fn a_report_reads_the_ledger_it_names_instead_of_claiming_facts() {
    let home = synthetic_root("empty-home");
    let state = synthetic_root("empty-state");
    let codex_home = synthetic_root("empty-codex");

    let reported = run_json(
        &home,
        &codex_home,
        &[
            "agent-usage",
            "report",
            "--state-root",
            &state.display().to_string(),
        ],
    );
    assert!(
        reported["metering"].get("available").is_none(),
        "an empty ledger is read, not refused: {}",
        reported["metering"]
    );
    assert_eq!(reported["metering"]["journal"], "usage-journal.v1");
    assert_eq!(
        reported["metering"]["store"],
        "graph-usage-ledger-v2.sqlite3"
    );
    assert_eq!(
        reported["metering"]["facts"].as_u64(),
        Some(0),
        "nothing was settled, so the projection claims no facts: {}",
        reported["metering"]
    );
    assert_eq!(
        reported["metering"]["obligations"]
            .as_array()
            .expect("obligations")
            .len(),
        0
    );
    assert_eq!(
        settled_fact_rows(&state),
        0,
        "the report read the ledger rather than settling facts of its own"
    );

    let _ = fs::remove_dir_all(&home);
    let _ = fs::remove_dir_all(&state);
    let _ = fs::remove_dir_all(&codex_home);
}
